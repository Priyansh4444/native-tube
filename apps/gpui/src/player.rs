//! The player: its own GPUI view, so a video frame re-renders only this view (the page, top bar
//! and rail are cached views that stay untouched).
//!
//! How frames reach the screen: mpv renders BGRA into a CPU buffer on the core's render thread
//! and runs our waker, which pings a bounded(1) channel. A task on this view drains it and
//! notifies the view only when the frame generation or a displayed part of the state changed.
//! On the next frame the newest picture is copied once into a `RenderImage`; GPUI uploads it to
//! its texture atlas when it is painted, and the previous picture's tile is freed with
//! `Window::drop_image`, so one video tile is alive at a time.

use crate::icons::icon;
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, App, Bounds, Context, ElementId, EventEmitter, Hsla, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    RenderImage, SharedString, Window, canvas, div, img, px,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;
use nt_core::{PixelFormat, PlaybackState, Player, Status, Video, format, metrics};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub enum PlayerEvent {
    ToggleTheatre,
    ToggleFullscreen,
    PlayNext(Box<Video>),
    AutoplayChanged(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Closed,
    Main,
    Speed,
    Quality,
}

#[derive(Clone, Copy, PartialEq)]
enum UpNext {
    Off,
    Counting { since: f64, generation: u64 },
    Cancelled,
}

pub const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];
const UP_NEXT_SECONDS: f64 = 5.0;
const HIDE_AFTER: f64 = 3.0;
/// The synthetic key used to make GPUI hide the pointer until it moves (see NOTES.md).
pub const HIDE_CURSOR_KEY: &str = "nt-hide-cursor";

fn now() -> f64 {
    metrics::now()
}

/// The parts of the playback state the UI shows; a change in any of them needs a frame.
type Shown = (
    Status,
    bool,
    bool,
    bool,
    i64,
    i64,
    i64,
    i64,
    i64,
    u32,
    usize,
    bool,
);

fn shown(s: &PlaybackState) -> Shown {
    (
        s.status.clone(),
        s.paused,
        s.buffering,
        s.muted,
        s.volume.round() as i64,
        s.duration.round() as i64,
        // The clock shows seconds; the scrubber moves with sub-second steps.
        (s.position * 4.0) as i64,
        (s.buffered_ahead * 2.0) as i64,
        (s.speed * 100.0).round() as i64,
        s.height,
        s.heights.len(),
        s.is_live,
    )
}

pub struct PlayerView {
    player: Option<Player>,
    /// `Player::new` failed; retried on the next load.
    start_error: Option<String>,
    pub state: PlaybackState,
    seen_gen: u64,
    shown_gen: u64,
    image: Option<Arc<RenderImage>>,
    /// The last picture of the video stopped by leaving its page, shown again (under the
    /// spinner) if Back returns to that same video, instead of a black box.
    parked: Option<(String, Arc<RenderImage>)>,
    to_drop: Vec<Arc<RenderImage>>,
    source: Option<String>,
    started_pending: bool,
    resume_at: Option<f64>,
    target: (u32, u32),
    next: Option<Video>,
    next_img: Option<Arc<RenderImage>>,
    next_req: u64,
    pub theatre: bool,
    pub fullscreen: bool,
    pub autoplay: bool,
    hovered: bool,
    last_move: f64,
    controls_awake: bool,
    hide_timer: bool,
    inhibit: crate::inhibit::Inhibit,
    /// The controls just hid under the pointer: hide the cursor too (done on the next render).
    cursor_hide: bool,
    menu: Menu,
    scrub_drag: Option<f32>,
    scrub_hover: Option<f32>,
    vol_drag: bool,
    bezel: Option<(u64, &'static str)>,
    bezel_seq: u64,
    up_next: UpNext,
    up_next_gen: u64,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    scrub_bounds: Rc<Cell<Bounds<Pixels>>>,
    vol_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<PlayerEvent> for PlayerView {}

impl PlayerView {
    pub fn new(autoplay: bool) -> PlayerView {
        PlayerView {
            player: None,
            start_error: None,
            state: PlaybackState::default(),
            seen_gen: 0,
            shown_gen: 0,
            image: None,
            parked: None,
            to_drop: Vec::new(),
            source: None,
            started_pending: false,
            resume_at: None,
            target: (0, 0),
            next: None,
            next_img: None,
            next_req: 0,
            theatre: false,
            fullscreen: false,
            autoplay,
            hovered: false,
            last_move: 0.0,
            controls_awake: false,
            hide_timer: false,
            inhibit: Default::default(),
            cursor_hide: false,
            menu: Menu::Closed,
            scrub_drag: None,
            scrub_hover: None,
            vol_drag: false,
            bezel: None,
            bezel_seq: 0,
            up_next: UpNext::Off,
            up_next_gen: 0,
            bounds: Rc::new(Cell::new(Bounds::default())),
            scrub_bounds: Rc::new(Cell::new(Bounds::default())),
            vol_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    /// The player, created on first use. A failed start is retried on the next call.
    fn ensure(&mut self, cx: &mut Context<Self>) -> Option<&Player> {
        if self.player.is_none() {
            let (tx, rx) = async_channel::bounded::<()>(1);
            match Player::new(PixelFormat::Bgra, move || {
                let _ = tx.try_send(());
            }) {
                Ok(p) => {
                    self.start_error = None;
                    self.player = Some(p);
                    cx.spawn(async move |this, cx| {
                        while rx.recv().await.is_ok() {
                            if this.update(cx, |p, cx| p.poll(cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                Err(e) => {
                    eprintln!("nt-gpui: player start failed: {e}");
                    self.start_error = Some(e.to_string());
                }
            }
        }
        self.player.as_ref()
    }

    /// Play `id` from the start or from `resume_at`. The previous picture is dropped at once.
    pub fn load(&mut self, id: &str, resume_at: Option<f64>, cx: &mut Context<Self>) {
        if let Some(old) = self.image.take() {
            self.to_drop.push(old);
        }
        match self.parked.take() {
            // Back to the same video: its own last picture, never another video's.
            Some((pid, img)) if pid == id => self.image = Some(img),
            Some((_, img)) => self.to_drop.push(img),
            None => {}
        }
        self.source = Some(id.to_string());
        self.resume_at = resume_at;
        self.up_next = UpNext::Off;
        self.menu = Menu::Closed;
        self.scrub_drag = None;
        self.state = PlaybackState {
            status: Status::Loading,
            ..self.state.clone()
        };
        self.start_error = None;
        let id = id.to_string();
        let result = self.ensure(cx).map(|p| p.load(&id));
        match result {
            Some(Ok(())) => {
                self.started_pending = true;
                if let Some(p) = &self.player {
                    self.shown_gen = p.frame_generation();
                }
            }
            Some(Err(e)) => self.state.status = Status::Failed(e.to_string()),
            None => {}
        }
        cx.notify();
    }

    /// Stop playback. Returns where it was, to resume later.
    pub fn stop(&mut self, cx: &mut Context<Self>) -> Option<(String, f64, f64)> {
        let at = self
            .source
            .take()
            .map(|s| (s, self.state.position, self.state.duration));
        if let Some(p) = &self.player {
            p.stop();
        }
        if let Some((_, img)) = self.parked.take() {
            self.to_drop.push(img);
        }
        if let (Some(img), Some((id, _, _))) = (self.image.take(), at.as_ref()) {
            self.parked = Some((id.clone(), img));
        }
        self.started_pending = false;
        self.up_next = UpNext::Off;
        self.menu = Menu::Closed;
        self.state.status = Status::Idle;
        self.inhibit.set(false, cx);
        cx.notify();
        at
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.source.clone() {
            let at = Some(self.state.position).filter(|p| *p > 1.0);
            self.load(&id, at, cx);
        }
    }

    pub fn set_next(&mut self, next: Option<Video>, cx: &mut Context<Self>) {
        if self.next.as_ref().map(|v| &v.id) == next.as_ref().map(|v| &v.id) {
            return;
        }
        if let Some(old) = self.next_img.take() {
            self.to_drop.push(old);
        }
        self.next_req += 1;
        let req = self.next_req;
        if let Some(v) = &next {
            let job = nt_core::images::image(&v.thumbnail, 320);
            cx.spawn(async move |this, cx| {
                let Ok(rgba) = job.await else { return };
                let img = cx
                    .background_executor()
                    .spawn(async move { crate::images::to_render_image(&rgba) })
                    .await;
                let _ = this.update(cx, |p, cx| {
                    if p.next_req == req {
                        p.next_img = img;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        self.next = next;
        cx.notify();
    }

    /// Called on every player wake.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.player else { return };
        let generation = p.frame_generation();
        let state = p.state();
        let changed = generation != self.seen_gen || shown(&state) != shown(&self.state);
        let was = self.state.status.clone();
        self.seen_gen = generation;
        self.state = state;
        if self.state.status == Status::Ready
            && let Some(at) = self.resume_at.take()
        {
            p.seek(at);
        }
        if self.state.status == Status::Ended && was != Status::Ended {
            self.on_ended(cx);
        }
        let playing = self.state.is_playing();
        self.inhibit.set(playing, cx);
        if changed {
            cx.notify();
        }
    }

    fn on_ended(&mut self, cx: &mut Context<Self>) {
        if !(self.autoplay && self.next.is_some()) {
            return;
        }
        self.up_next_gen += 1;
        let generation = self.up_next_gen;
        self.up_next = UpNext::Counting {
            since: now(),
            generation,
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                let go = this.update(cx, |p, cx| {
                    let UpNext::Counting {
                        since,
                        generation: g,
                    } = p.up_next
                    else {
                        return Some(false);
                    };
                    if g != generation {
                        return Some(false);
                    }
                    if now() - since >= UP_NEXT_SECONDS - 0.05 {
                        p.up_next = UpNext::Off;
                        if let Some(v) = p.next.clone() {
                            cx.emit(PlayerEvent::PlayNext(Box::new(v)));
                        }
                        return Some(false);
                    }
                    cx.notify();
                    Some(true)
                });
                if !matches!(go, Ok(Some(true))) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    // ---- commands (buttons, keys and the autopilot all land here) ----------------------------

    pub fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.player {
            if self.state.status == Status::Ended {
                self.up_next = UpNext::Cancelled;
            }
            let icon = if self.state.paused || self.state.status == Status::Ended {
                "play_arrow"
            } else {
                "pause"
            };
            p.toggle_pause();
            self.bezel_seq += 1;
            self.bezel = Some((self.bezel_seq, icon));
        }
        cx.notify();
    }

    pub fn seek_by(&mut self, s: f64, cx: &mut Context<Self>) {
        if let Some(p) = &self.player
            && !self.state.is_live
        {
            p.seek_by(s);
            self.wake_controls(cx);
        }
    }

    pub fn seek_fraction(&mut self, f: f64, cx: &mut Context<Self>) {
        let d = self.state.duration;
        if let Some(p) = &self.player
            && d > 0.0
            && !self.state.is_live
        {
            p.seek(f.clamp(0.0, 1.0) * d);
            if self.state.status == Status::Ended {
                self.up_next = UpNext::Cancelled;
            }
            self.wake_controls(cx);
        }
    }

    pub fn frame_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.state.paused {
            // The core has no frame-step command; an exact seek of one 30 fps frame is the same
            // on most YouTube videos.
            self.seek_by(if forward { 1.0 / 30.0 } else { -1.0 / 30.0 }, cx);
        }
    }

    pub fn nudge_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(p) = &self.player {
            let v = (self.state.volume + delta).clamp(0.0, 100.0);
            p.set_volume(v);
            p.set_muted(false);
            self.state.volume = v;
            self.wake_controls(cx);
        }
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.player {
            p.set_muted(!self.state.muted);
        }
        cx.notify();
    }

    fn set_volume_at(&mut self, x: f32, cx: &mut Context<Self>) {
        let b = self.vol_bounds.get();
        let w = f32::from(b.size.width).max(1.0);
        let f = ((x - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
        if let Some(p) = &self.player {
            p.set_volume(f as f64 * 100.0);
            p.set_muted(f <= 0.0);
            self.state.volume = f as f64 * 100.0;
        }
        cx.notify();
    }

    pub fn step_speed(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.state.is_live {
            return;
        }
        let cur = if self.state.speed > 0.0 {
            self.state.speed
        } else {
            1.0
        };
        let i = SPEEDS
            .iter()
            .position(|s| (s - cur).abs() < 0.01)
            .unwrap_or(3);
        let j = if up {
            (i + 1).min(SPEEDS.len() - 1)
        } else {
            i.saturating_sub(1)
        };
        if let (Some(p), Some(s)) = (&self.player, SPEEDS.get(j)) {
            p.set_speed(*s);
        }
        self.wake_controls(cx);
    }

    pub fn play_next(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.next.clone() {
            cx.emit(PlayerEvent::PlayNext(Box::new(v)));
        }
    }

    /// Show the controls now, and hide them (with the cursor) after 3 s without movement.
    fn wake_controls(&mut self, cx: &mut Context<Self>) {
        self.last_move = now();
        if !self.controls_awake {
            self.controls_awake = true;
            cx.notify();
        }
        if self.hide_timer {
            return;
        }
        self.hide_timer = true;
        cx.spawn(async move |this, cx| {
            let mut wait = HIDE_AFTER;
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(wait.max(0.05)))
                    .await;
                let next = this.update(cx, |p, cx| {
                    let left = HIDE_AFTER - (now() - p.last_move);
                    if left > 0.02 {
                        return Some(left);
                    }
                    p.hide_timer = false;
                    if p.controls_awake {
                        p.controls_awake = false;
                        // Only while playing: paused controls stay, and so does the pointer.
                        p.cursor_hide = p.hovered && p.state.is_playing() && p.menu == Menu::Closed;
                        cx.notify();
                    }
                    None
                });
                match next {
                    Ok(Some(left)) => wait = left,
                    _ => break,
                }
            }
        })
        .detach();
    }

    fn controls_visible(&self) -> bool {
        let busy = self.menu != Menu::Closed
            || self.scrub_drag.is_some()
            || self.vol_drag
            || self.state.paused
            || !matches!(self.state.status, Status::Ready);
        busy || (self.controls_awake && (self.hovered || self.fullscreen))
    }

    fn scrub_fraction(&self, x: f32) -> f32 {
        let b = self.scrub_bounds.get();
        let w = f32::from(b.size.width).max(1.0);
        ((x - f32::from(b.origin.x)) / w).clamp(0.0, 1.0)
    }

    fn on_drag_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let x = f32::from(ev.position.x);
        if self.scrub_drag.is_some() {
            self.scrub_drag = Some(self.scrub_fraction(x));
            cx.notify();
        } else if self.vol_drag {
            self.set_volume_at(x, cx);
        }
    }

    fn on_drag_end(&mut self, cx: &mut Context<Self>) {
        // Seek once, on release.
        if let Some(f) = self.scrub_drag.take() {
            self.seek_fraction(f as f64, cx);
        }
        self.vol_drag = false;
        cx.notify();
    }

    // ---- rendering ---------------------------------------------------------------------------

    fn upload(&mut self, window: &mut Window) -> bool {
        for old in self.to_drop.drain(..) {
            let _ = window.drop_image(old);
        }
        let Some(p) = &self.player else { return false };
        let generation = p.frame_generation();
        if generation == self.shown_gen {
            return false;
        }
        let Some(frame) = p.frame() else {
            // The core cleared its frame (load or stop): never keep showing the old picture.
            if let Some(old) = self.image.take() {
                let _ = window.drop_image(old);
            }
            return false;
        };
        self.shown_gen = generation;
        let Some(buf) =
            image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels().to_vec())
        else {
            return false;
        };
        let new = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buf
        )]));
        if let Some(old) = self.image.replace(new) {
            let _ = window.drop_image(old);
        }
        true
    }

    fn icon_button(
        &self,
        id: &'static str,
        name: &'static str,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .size(px(40.0))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(fill()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                on(this, window, cx);
            }))
            .child(icon(name, 24.0, c(0xffffff)))
    }

    fn scrubber(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.state;
        let d = s.duration;
        let frac = self
            .scrub_drag
            .unwrap_or(if d > 0.0 {
                (s.position / d) as f32
            } else {
                0.0
            })
            .clamp(0.0, 1.0);
        let buffered = if d > 0.0 {
            ((s.position + s.buffered_ahead) / d).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let live = s.is_live;
        let bounds = self.scrub_bounds.clone();
        let tip = self.scrub_drag.or(self.scrub_hover);
        let tip_el = tip.filter(|_| d > 0.0).map(|f| {
            div()
                .absolute()
                .bottom(px(18.0))
                .left(gpui::relative(f))
                .ml(px(-24.0))
                .w(px(48.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(badge_bg())
                        .child(txt(format::clock(f as f64 * d), 12.0, 18.0, W500, text())),
                )
        });
        div()
            .id("scrub")
            .group("scrub")
            .relative()
            .h(px(16.0))
            .mx(px(12.0))
            .flex()
            .items_end()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.state.is_live || this.state.duration <= 0.0 {
                        return;
                    }
                    this.scrub_drag = Some(this.scrub_fraction(f32::from(ev.position.x)));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let f = this.scrub_fraction(f32::from(ev.position.x));
                if this.scrub_hover != Some(f) {
                    this.scrub_hover = Some(f);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.scrub_hover.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(3.0))
                    .mb(px(2.0))
                    .group_hover("scrub", |s| s.h(px(5.0)).mb(px(1.0)))
                    .bg(fill_hover())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(buffered))
                            .bg(gpui::rgba(0xffffff66)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(if live { 1.0 } else { frac }))
                            .bg(red()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(-5.0))
                            .left(gpui::relative(frac))
                            .ml(px(-6.5))
                            .size(px(13.0))
                            .rounded_full()
                            .bg(red())
                            .when(self.scrub_drag.is_none(), |d| {
                                d.opacity(0.0).group_hover("scrub", |s| s.opacity(1.0))
                            }),
                    ),
            )
            .children(tip_el)
    }

    fn volume(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.state;
        let name = if s.muted || s.volume <= 0.0 {
            "volume_off"
        } else if s.volume < 50.0 {
            "volume_down"
        } else {
            "volume_up"
        };
        let level = if s.muted {
            0.0
        } else {
            (s.volume / 100.0) as f32
        };
        let bounds = self.vol_bounds.clone();
        let open = self.vol_drag;
        div()
            .id("volume")
            .group("vol")
            .flex()
            .items_center()
            .child(self.icon_button("mute", name, cx, |this, _, cx| this.toggle_mute(cx)))
            .child(
                div()
                    .id("vol-slider")
                    .h(px(40.0))
                    .w(px(if open { 60.0 } else { 0.0 }))
                    .group_hover("vol", |s| s.w(px(60.0)))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.vol_drag = true;
                            this.set_volume_at(f32::from(ev.position.x), cx);
                        }),
                    )
                    .child(
                        div()
                            .relative()
                            .ml(px(2.0))
                            .w(px(52.0))
                            .h(px(3.0))
                            .bg(fill_hover())
                            .child(
                                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui::relative(level))
                                    .bg(c(0xffffff)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-4.5))
                                    .left(gpui::relative(level))
                                    .ml(px(-6.0))
                                    .size(px(12.0))
                                    .rounded_full()
                                    .bg(c(0xffffff)),
                            ),
                    ),
            )
    }

    fn menu_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.menu == Menu::Closed {
            return None;
        }
        let s = &self.state;
        let row = |id: &'static str,
                   left: Option<&'static str>,
                   label: SharedString,
                   value: Option<SharedString>,
                   check: bool| {
            div()
                .id(id)
                .h(px(40.0))
                .px(px(16.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(fill()))
                .when_some(left, |d, name| d.child(icon(name, 24.0, text())))
                .when(left.is_none(), |d| {
                    d.child(
                        div()
                            .w(px(24.0))
                            .flex_none()
                            .when(check, |d| d.child(icon("check", 24.0, text()))),
                    )
                })
                .child(txt(label, 14.0, 20.0, W500, text()).flex_1())
                .when_some(value, |d, v| {
                    d.child(txt(v, 14.0, 20.0, W400, text())).child(icon(
                        "chevron_right",
                        24.0,
                        text(),
                    ))
                })
        };
        let mut panel = div()
            .id("player-menu")
            .absolute()
            .right(px(12.0))
            .bottom(px(60.0))
            .w(px(260.0))
            .max_h(px(
                (f32::from(self.bounds.get().size.height) - 72.0).max(120.0)
            ))
            .overflow_hidden()
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(gpui::rgba(0x282828ee))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match self.menu {
            Menu::Main => {
                if !s.is_live {
                    let speed = if (s.speed - 1.0).abs() < 0.01 || s.speed == 0.0 {
                        "Normal".to_string()
                    } else {
                        format!("{}", s.speed)
                    };
                    panel = panel.child(
                        row(
                            "m-speed",
                            Some("speed"),
                            "Playback speed".into(),
                            Some(speed.into()),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = Menu::Speed;
                            cx.notify();
                        })),
                    );
                }
                let q = match self.player.as_ref().and_then(|p| p.quality()) {
                    Some(h) => format!("{h}p"),
                    None if s.height > 0 => format!("Auto ({}p)", s.height),
                    None => "Auto".to_string(),
                };
                panel = panel.child(
                    row(
                        "m-quality",
                        Some("tune"),
                        "Quality".into(),
                        Some(q.into()),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = Menu::Quality;
                        cx.notify();
                    })),
                );
            }
            Menu::Speed => {
                panel = panel.child(self.menu_header("Playback speed", cx));
                for (i, sp) in SPEEDS.iter().enumerate() {
                    let label: SharedString = if *sp == 1.0 {
                        "Normal".into()
                    } else {
                        format!("{sp}").into()
                    };
                    let current = (s.speed - sp).abs() < 0.01 || (s.speed == 0.0 && *sp == 1.0);
                    let sp = *sp;
                    panel = panel.child(
                        row(
                            SPEED_IDS.get(i).copied().unwrap_or("sp"),
                            None,
                            label,
                            None,
                            current,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = &this.player {
                                p.set_speed(sp);
                            }
                            this.menu = Menu::Closed;
                            cx.notify();
                        })),
                    );
                }
            }
            Menu::Quality => {
                panel = panel.child(self.menu_header("Quality", cx));
                let chosen = self.player.as_ref().and_then(|p| p.quality());
                let mut heights = s.heights.clone();
                heights.sort_unstable_by(|a, b| b.cmp(a));
                heights.dedup();
                for (i, h) in heights.into_iter().enumerate() {
                    panel = panel.child(
                        row(
                            QUALITY_IDS.get(i).copied().unwrap_or("q"),
                            None,
                            format!("{h}p").into(),
                            None,
                            chosen == Some(h),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = &this.player {
                                p.set_quality(Some(h));
                            }
                            this.menu = Menu::Closed;
                            cx.notify();
                        })),
                    );
                }
                let auto = if s.height > 0 && chosen.is_none() {
                    format!("Auto ({}p)", s.height)
                } else {
                    "Auto".to_string()
                };
                panel = panel.child(
                    row("q-auto", None, auto.into(), None, chosen.is_none()).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(p) = &this.player {
                                p.set_quality(None);
                            }
                            this.menu = Menu::Closed;
                            cx.notify();
                        },
                    )),
                );
            }
            Menu::Closed => {}
        }
        Some(panel)
    }

    fn menu_header(&self, title: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("menu-back")
            .h(px(48.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(divider())
            .mb(px(8.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = Menu::Main;
                cx.notify();
            }))
            .child(icon("chevron_left", 24.0, text()))
            .child(txt(title, 14.0, 20.0, W500, text()))
    }

    fn control_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.state;
        let ended = s.status == Status::Ended;
        let play_icon = if ended {
            "replay"
        } else if s.paused {
            "play_arrow"
        } else {
            "pause"
        };
        let time: gpui::AnyElement = if s.is_live {
            div()
                .ml(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().size(px(8.0)).rounded_full().bg(red()))
                .child(txt("LIVE", 13.0, 18.0, W500, text()))
                .into_any_element()
        } else {
            txt(
                format!(
                    "{} / {}",
                    format::clock(
                        self.scrub_drag
                            .map_or(s.position, |f| f as f64 * s.duration)
                    ),
                    format::clock(s.duration)
                ),
                13.0,
                18.0,
                W400,
                c(0xeeeeee),
            )
            .ml(px(8.0))
            .whitespace_nowrap()
            .into_any_element()
        };
        let autoplay = self.autoplay;
        div()
            .h(px(48.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .child(self.icon_button("play", play_icon, cx, |this, _, cx| this.toggle_pause(cx)))
            .when(self.next.is_some(), |d| {
                d.child(self.icon_button("next", "skip_next", cx, |this, _, cx| this.play_next(cx)))
            })
            .child(self.volume(cx))
            .child(time)
            .child(div().flex_1())
            .child(
                div()
                    .id("autoplay")
                    .h(px(40.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.autoplay = !this.autoplay;
                        if !this.autoplay && matches!(this.up_next, UpNext::Counting { .. }) {
                            this.up_next = UpNext::Cancelled;
                        }
                        cx.emit(PlayerEvent::AutoplayChanged(this.autoplay));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .relative()
                            .w(px(36.0))
                            .h(px(14.0))
                            .rounded_full()
                            .bg(if autoplay { c(0xaaaaaa) } else { fill_hover() })
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-3.0))
                                    .left(px(if autoplay { 16.0 } else { 0.0 }))
                                    .size(px(20.0))
                                    .rounded_full()
                                    .bg(c(0xffffff))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(
                                        if autoplay { "play_arrow" } else { "pause" },
                                        14.0,
                                        black(),
                                    )),
                            ),
                    ),
            )
            .child(self.icon_button("settings", "settings", cx, |this, _, cx| {
                this.menu = if this.menu == Menu::Closed {
                    Menu::Main
                } else {
                    Menu::Closed
                };
                cx.notify();
            }))
            .when(!self.fullscreen, |d| {
                d.child(self.icon_button("theatre", "width_wide", cx, |_, _, cx| {
                    cx.emit(PlayerEvent::ToggleTheatre)
                }))
            })
            .child(self.icon_button(
                "fullscreen",
                if self.fullscreen {
                    "fullscreen_exit"
                } else {
                    "fullscreen"
                },
                cx,
                |_, _, cx| cx.emit(PlayerEvent::ToggleFullscreen),
            ))
    }

    fn up_next_card(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let UpNext::Counting { since, .. } = self.up_next else {
            return None;
        };
        let v = self.next.as_ref()?;
        let left = (UP_NEXT_SECONDS - (now() - since)).ceil().max(0.0) as i64;
        let thumb = div()
            .w(px(240.0))
            .h(px(135.0))
            .rounded(px(8.0))
            .bg(fill())
            .when_some(self.next_img.clone(), |d, i| {
                d.child(
                    img(i)
                        .size_full()
                        .rounded(px(8.0))
                        .object_fit(ObjectFit::Cover),
                )
            });
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(gpui::rgba(0x000000b3))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .w(px(240.0))
                        .child(txt(format!("Up next in {left}"), 14.0, 20.0, W400, text2()))
                        .child(thumb)
                        .child(lines(v.title.clone(), 16.0, 22.0, W500, text(), 2))
                        .child(line(v.channel.clone(), 14.0, 20.0, W400, text2()))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("upnext-cancel")
                                        .flex_1()
                                        .h(px(36.0))
                                        .rounded(px(18.0))
                                        .bg(fill())
                                        .hover(|s| s.bg(fill_hover()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.up_next = UpNext::Cancelled;
                                            cx.notify();
                                        }))
                                        .child(txt("Cancel", 14.0, 20.0, W500, text())),
                                )
                                .child(
                                    div()
                                        .id("upnext-play")
                                        .flex_1()
                                        .h(px(36.0))
                                        .rounded(px(18.0))
                                        .bg(text())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.up_next = UpNext::Off;
                                            this.play_next(cx);
                                        }))
                                        .child(txt("Play now", 14.0, 20.0, W500, on_light())),
                                ),
                        ),
                ),
        )
    }

    fn overlay_message(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let failed = self.start_error.is_some() || matches!(self.state.status, Status::Failed(_));
        if !failed {
            return None;
        }
        if let Status::Failed(e) = &self.state.status {
            eprintln!("nt-gpui: playback failed: {e}");
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(black())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .p(px(24.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon("error", 48.0, text2()))
                .child(txt("This video can't be played", 20.0, 28.0, W700, text()))
                .child(txt(
                    "Something went wrong while loading it. Try again.",
                    14.0,
                    20.0,
                    W400,
                    text2(),
                ))
                .child(
                    div()
                        .id("player-retry")
                        .h(px(36.0))
                        .px(px(16.0))
                        .rounded(px(18.0))
                        .bg(text())
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
                        .child(txt("Retry", 14.0, 20.0, W500, on_light())),
                ),
        )
    }
}

const SPEED_IDS: [&str; 8] = ["sp0", "sp1", "sp2", "sp3", "sp4", "sp5", "sp6", "sp7"];
const QUALITY_IDS: [&str; 12] = [
    "q0", "q1", "q2", "q3", "q4", "q5", "q6", "q7", "q8", "q9", "q10", "q11",
];

fn bezel(seq: u64, name: &'static str) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(52.0))
                .rounded_full()
                .bg(gpui::rgba(0x00000099))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(name, 36.0, c(0xffffff))),
        )
        .with_animation(
            ElementId::NamedInteger("bezel".into(), seq),
            Animation::new(Duration::from_millis(500)),
            |el, t| el.opacity(1.0 - t),
        )
}

fn spinner_overlay(color: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(Spinner::new().large().color(color))
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let new_picture = self.upload(window);
        if std::mem::take(&mut self.cursor_hide) {
            // Not `cx.defer_in`: that runs inside this view's update, and dispatching a key
            // draws the window first when it is dirty, which renders this view again (the
            // "already being updated" panic). `Window::defer` holds no entity.
            window.defer(cx, hide_cursor);
        }
        let mark = new_picture && std::mem::take(&mut self.started_pending);
        let s = &self.state;
        let failed = self.start_error.is_some() || matches!(s.status, Status::Failed(_));
        let loading = !failed
            && self.source.is_some()
            && (matches!(s.status, Status::Loading) || s.buffering);
        let show = self.controls_visible() && self.source.is_some() && !failed;
        let rounded = !(self.theatre || self.fullscreen);
        let me = cx.entity().downgrade();
        let bounds = self.bounds.clone();
        let target = self.target;
        let scale = window.scale_factor();
        let dragging = self.scrub_drag.is_some() || self.vol_drag;
        // Bounds, target size, drag tracking outside the player, and the playback mark.
        let hook = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, cx| {
                let w = (f32::from(b.size.width) * scale).round() as u32;
                let h = (f32::from(b.size.height) * scale).round() as u32;
                if (w, h) != target && w > 0 && h > 0 {
                    let _ = me.update(cx, |p, _| {
                        p.target = (w, h);
                        if let Some(pl) = &p.player {
                            pl.set_target_size(w, h);
                        }
                    });
                }
                if mark {
                    metrics::mark(metrics::marks::PLAYBACK_STARTED);
                }
                if dragging {
                    let m1 = me.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
                        let _ = m1.update(cx, |p, cx| p.on_drag_move(ev, cx));
                    });
                    let m2 = me.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                        let _ = m2.update(cx, |p, cx| p.on_drag_end(cx));
                    });
                }
            },
        )
        .absolute()
        .inset_0();

        let mut root = div()
            .id("player")
            .relative()
            .size_full()
            .bg(black())
            .when(rounded, |d| d.rounded(px(12.0)))
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| this.wake_controls(cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                if *hovered {
                    this.wake_controls(cx);
                } else {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if this.menu != Menu::Closed {
                        this.menu = Menu::Closed;
                        cx.notify();
                        return;
                    }
                    if this.source.is_none() {
                        return;
                    }
                    this.toggle_pause(cx);
                    if ev.click_count == 2 {
                        // A double click toggles fullscreen; the second click undid the pause.
                        cx.emit(PlayerEvent::ToggleFullscreen);
                    }
                }),
            )
            .child(hook);
        if let Some(frame) = self.image.clone() {
            root = root.child(
                img(frame)
                    .size_full()
                    .when(rounded, |d| d.rounded(px(12.0)))
                    .object_fit(ObjectFit::Contain),
            );
        }
        if loading {
            root = root.child(spinner_overlay(text()));
        }
        if let Some((seq, name)) = self.bezel {
            root = root.child(bezel(seq, name));
        }
        if show {
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .pt(px(48.0))
                    .bg(gpui::linear_gradient(
                        0.0,
                        gpui::linear_color_stop(gpui::rgba(0x00000099), 0.0),
                        gpui::linear_color_stop(gpui::rgba(0x00000000), 1.0),
                    ))
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.scrubber(cx))
                    .child(self.control_bar(cx)),
            );
            root = root.children(self.menu_panel(cx));
        }
        root = root.children(self.up_next_card(cx));
        root = root.children(self.overlay_message(cx));
        root
    }
}

/// Make GPUI hide the pointer until it next moves: a synthetic, inert keystroke trips GPUI's
/// "hide while typing" path (`CursorHideMode::OnTypingAndAction`). The root key handler swallows
/// it. GPUI has no public "hide cursor" call.
pub fn hide_cursor(window: &mut Window, cx: &mut App) {
    let ks = Keystroke {
        modifiers: Modifiers::default(),
        key: HIDE_CURSOR_KEY.to_string(),
        key_char: Some(String::new()),
    };
    window.dispatch_keystroke(ks, cx);
}
