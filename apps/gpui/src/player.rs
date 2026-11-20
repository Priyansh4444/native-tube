//! The player: its own GPUI view, so a video frame re-renders only this view (the page, top bar
//! and rail are cached views that stay untouched).
//!
//! How frames reach the screen: mpv renders BGRA into a CPU buffer on the core's render thread
//! and runs our waker, which pings a bounded(1) channel. A task on this view drains it and
//! notifies the view only when the frame generation or a displayed part of the state changed.
//! On the next frame the newest picture is copied once into a `RenderImage`; GPUI uploads it to
//! its texture atlas when it is painted, and the previous picture's tile is freed with
//! `Window::drop_image`, so one video tile is alive at a time.

use crate::chrome::focus_ring;
use crate::icons::icon;
use crate::probe::{self, Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, App, Bounds, Context, ElementId, EventEmitter, Hsla, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    RenderImage, SharedString, Window, canvas, div, img, px,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;
use nt_core::{
    Chapter, HeatPoint, PixelFormat, PlaybackState, Player, Status, Video, format, metrics,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub enum PlayerEvent {
    ToggleTheatre,
    ToggleFullscreen,
    PlayNext(Box<Video>),
    PlayPrevious(Box<Video>),
    AutoplayChanged(bool),
    /// A short confirmation for the app's toast ("Link copied").
    Toast(&'static str),
    ShowChapters,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Closed,
    Main,
    Speed,
    Quality,
    Captions,
    Translate,
    Sleep,
    Audio,
}

/// The sleep timer: pauses playback when it fires.
#[derive(Clone, Copy, PartialEq)]
pub enum Sleep {
    Off,
    /// Fires at this `now()`; the chosen length in minutes.
    At(f64, u32),
    EndOfVideo,
}

pub const SLEEP_MINUTES: [u32; 6] = [10, 15, 20, 30, 45, 60];

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

/// `NT_TRACE=1`: one `NT_PLAY <ms> <event> <video>` line per step of a video start, to break
/// a click → first picture down.
fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NT_TRACE").is_some())
}

pub fn trace(event: &str, id: &str) {
    if tracing() {
        eprintln!("NT_PLAY {:.1} {event} {id}", now() * 1000.0);
    }
}

/// The parts of the playback state the UI shows; a change in any of them needs a frame.
type Shown = (
    (
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
    ),
    (usize, Option<(usize, Option<String>)>),
    (usize, Option<String>, bool, bool, bool),
);

fn shown(s: &PlaybackState) -> Shown {
    (
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
        ),
        (s.captions.len(), s.selected_captions.clone()),
        (
            s.audio_tracks.len(),
            s.audio_track.clone(),
            s.normalize,
            s.looping,
            s.normalization_supported,
        ),
    )
}

pub struct PlayerView {
    // Field order ensures the lease drops before the player on abnormal window teardown.
    #[cfg(target_os = "linux")]
    surface: Option<crate::hardware::Surface>,
    #[cfg(target_os = "linux")]
    hardware_attempted: bool,
    hardware_presenting: bool,
    native_paint: bool,
    player: Option<Player>,
    /// `Player::new` failed; retried on the next load.
    start_error: Option<String>,
    pub state: PlaybackState,
    seen_gen: u64,
    shown_gen: u64,
    image: Option<Arc<RenderImage>>,
    /// The last picture of the video stopped by leaving its page, shown again (under the
    /// spinner) if Back returns to that same video, instead of a black box.
    parked: Vec<(String, Arc<RenderImage>)>,
    /// The video's own thumbnail, at the player's width, shown under the spinner until its first
    /// frame arrives (e.g. after rapid Back presses land on a watch page).
    poster: Option<(String, Arc<RenderImage>)>,
    poster_task: Option<gpui::Task<()>>,
    poster_fallback: Option<Arc<RenderImage>>,
    /// The window's scale factor at the last draw, for image requests in physical pixels.
    scale: f32,
    to_drop: Vec<Arc<RenderImage>>,
    source: Option<String>,
    started_pending: bool,
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    presented: Rc<Cell<bool>>,
    source_epoch: u64,
    preview_key: Option<nt_core::StoryboardFrame>,
    preview_task: Option<gpui::Task<()>>,
    preview_image: Option<Arc<RenderImage>>,
    stats_sample: (f64, u64),
    render_fps: f64,
    target: (u32, u32),
    next: Option<Video>,
    previous: Option<Video>,
    next_img: Option<Arc<RenderImage>>,
    next_req: u64,
    pub theatre: bool,
    /// Caption lift: 0 = resting near the picture's bottom, 1 = above the controls. Animated
    /// over 200 ms (from, to, start) when the controls show or hide (core §C7).
    caption_lift: f32,
    caption_anim: Option<(f32, f32, f64)>,
    /// The last `sub-pos` sent to the player (NaN: none yet).
    caption_pos: f64,
    /// Test-only (`NT_TEST_CYCLE=controls`): controls forced shown/hidden in turns.
    test_controls: Option<bool>,
    test_started: bool,
    test_cc_done: bool,
    test_fullscreen_done: bool,
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
    /// The right-click menu, at player-local coordinates.
    context: Option<(f32, f32)>,
    /// Stats for nerds.
    pub stats: bool,
    pub sleep: Sleep,
    sleep_task: Option<gpui::Task<()>>,
    chapters: Vec<Chapter>,
    heat: Vec<HeatPoint>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    scrub_bounds: Rc<Cell<Bounds<Pixels>>>,
    vol_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<PlayerEvent> for PlayerView {}

impl PlayerView {
    pub fn new(autoplay: bool) -> PlayerView {
        PlayerView {
            #[cfg(target_os = "linux")]
            surface: None,
            #[cfg(target_os = "linux")]
            hardware_attempted: false,
            hardware_presenting: false,
            native_paint: false,
            player: None,
            start_error: None,
            state: PlaybackState::default(),
            seen_gen: 0,
            shown_gen: 0,
            image: None,
            parked: Vec::new(),
            poster: None,
            poster_task: None,
            poster_fallback: None,
            scale: 1.0,
            to_drop: Vec::new(),
            source: None,
            started_pending: false,
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            presented: Rc::new(Cell::new(false)),
            source_epoch: 0,
            preview_key: None,
            preview_task: None,
            preview_image: None,
            stats_sample: (0.0, 0),
            render_fps: 0.0,
            target: (0, 0),
            next: None,
            previous: None,
            next_img: None,
            next_req: 0,
            theatre: false,
            caption_lift: 0.0,
            caption_anim: None,
            caption_pos: f64::NAN,
            test_controls: None,
            test_started: false,
            test_cc_done: false,
            test_fullscreen_done: false,
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
            context: None,
            stats: false,
            sleep: Sleep::Off,
            sleep_task: None,
            chapters: Vec::new(),
            heat: Vec::new(),
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
                    p.set_target_size(self.target.0, self.target.1);
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

    pub fn attach_hardware(&mut self, window: &Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if !self.hardware_attempted {
            if std::env::var_os("NT_SOFTWARE_VIDEO").is_some() {
                self.hardware_attempted = true;
                return;
            }
            let Some(player) = self.ensure(cx) else {
                return;
            };
            let surface = crate::hardware::Surface::attach(player, window);
            self.hardware_attempted = true;
            match surface {
                Ok(surface) => {
                    eprintln!("nt-gpui: attached Wayland GPU surface");
                    self.surface = Some(surface);
                }
                Err(e) => eprintln!("nt-gpui: using software video: {e}"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (window, cx);
    }

    pub fn sync_hardware(
        &mut self,
        rect: Option<Bounds<Pixels>>,
        area: Bounds<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        #[cfg(target_os = "linux")]
        {
            if let Some(surface) = &mut self.surface {
                surface.update(rect, area, window.scale_factor());
            }
            if let Some(error) = self
                .surface
                .as_ref()
                .and_then(crate::hardware::Surface::error)
            {
                eprintln!("nt-gpui: GPU playback fell back to software: {error}");
                self.detach_hardware();
                window.set_background_appearance(gpui::WindowBackgroundAppearance::Opaque);
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (window, area);
        let native_paint = rect.is_some() && self.native_frame_available();
        if self.native_paint != native_paint {
            self.native_paint = native_paint;
            cx.notify();
        }
        native_paint
    }

    pub fn detach_hardware(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(surface) = self.surface.take() {
            surface.detach();
        }
        self.hardware_presenting = false;
        self.native_paint = false;
    }

    fn native_frame_available(&self) -> bool {
        #[cfg(target_os = "linux")]
        if matches!(self.state.status, Status::Ready | Status::Ended)
            && let (Some(surface), Some(player)) = (&self.surface, &self.player)
        {
            // A real picture of this file, not the black render the surface maps with (or a
            // resize-only render before the first picture): `presented_frames` counts only
            // decoded frames since this load's playback restart. Until then the poster or the
            // last frame stays.
            return surface.is_presenting() && player.presented_frames() > 0;
        }
        false
    }

    /// Play `id` from the start or from `start` seconds. The previous picture is dropped at once;
    /// until the first new frame the video's own last picture (Back) or its thumbnail shows.
    pub fn load(
        &mut self,
        id: &str,
        start: Option<f64>,
        thumbnail: Option<(&str, f32)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(old) = self.image.take() {
            self.to_drop.push(old);
        }
        if let Some(i) = self.parked.iter().position(|(pid, _)| pid == id) {
            self.image = Some(self.parked.remove(i).1);
        }
        if let Some((pid, img)) = self.poster.take() {
            if pid == id {
                self.poster = Some((pid, img));
            } else {
                self.to_drop.push(img);
            }
        }
        self.source_epoch += 1;
        self.poster_task = None;
        self.hardware_presenting = false;
        self.native_paint = false;
        self.presented.set(false);
        self.clear_preview();
        self.started_pending = false;
        self.scrub_hover = None;
        self.bezel = None;
        self.vol_drag = false;
        self.controls_awake = true;
        self.source = Some(id.to_string());
        self.up_next = UpNext::Off;
        self.menu = Menu::Closed;
        self.context = None;
        self.scrub_drag = None;
        self.last_move = now();
        self.state = PlaybackState {
            status: Status::Loading,
            position: start.unwrap_or(0.0),
            muted: self.state.muted,
            volume: self.state.volume,
            ..PlaybackState::default()
        };
        self.start_error = None;
        let id = id.to_string();
        let at = start.unwrap_or(0.0);
        trace("load", &id);
        let result = self.ensure(cx).map(|p| {
            let before = p.frame_generation();
            (before, p.load_at(&id, at))
        });
        trace("load-return", &id);
        match result {
            Some((before, Ok(()))) => {
                self.started_pending = true;
                self.shown_gen = before;
                if let Some(player) = &self.player {
                    self.state = player.state();
                }
                if self.image.is_none()
                    && let Some((url, width)) = thumbnail
                {
                    self.request_poster(&id, url, width, cx);
                }
            }
            Some((_, Err(e))) => self.state.status = Status::Failed(e.to_string()),
            None => {}
        }
        cx.notify();
    }

    pub fn set_target(&mut self, (w, h): (u32, u32)) {
        if self.target != (w, h) && w > 0 && h > 0 {
            self.target = (w, h);
            if let Some(p) = &self.player {
                p.set_target_size(w, h);
            }
            self.prepare_next();
        }
    }

    pub fn set_poster_fallback(&mut self, fallback: Option<Arc<RenderImage>>) {
        self.poster_fallback = fallback;
    }

    /// A visible card provides an immediate poster. Upgrade it only if playback stalls.
    fn request_poster(&mut self, id: &str, url: &str, width: f32, cx: &mut Context<Self>) {
        if url.is_empty() || self.poster.as_ref().is_some_and(|(pid, _)| pid == id) {
            return;
        }
        let delay = self.poster_fallback.is_some();
        let epoch = self.source_epoch;
        let id = id.to_string();
        let url = url.to_string();
        let width = crate::images::bucket(width);
        self.poster_task = Some(cx.spawn(async move |this, cx| {
            if delay {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
            let Ok(rgba) = nt_core::images::image(&url, width).await else {
                return;
            };
            let img = cx
                .background_executor()
                .spawn(async move { crate::images::to_render_image(&rgba) })
                .await;
            let _ = this.update(cx, |p, cx| {
                if p.source_epoch == epoch
                    && !p.presented.get()
                    && let Some(img) = img
                {
                    p.poster = Some((id, img));
                    cx.notify();
                }
            });
        }));
    }

    /// Stop playback. Returns where it was, to resume later.
    pub fn stop(&mut self, unload: bool, cx: &mut Context<Self>) -> Option<(String, f64, f64)> {
        let at = self
            .source
            .take()
            .map(|s| (s, self.state.position, self.state.duration));
        if unload && let Some(p) = &self.player {
            p.stop();
        }
        self.source_epoch += 1;
        self.clear_preview();
        if let Some((_, img)) = self.poster.take() {
            self.to_drop.push(img);
        }
        self.poster_task = None;
        self.poster_fallback = None;
        if let (Some(img), Some((id, _, _))) = (self.image.take(), at.as_ref()) {
            self.parked.push((id.clone(), img));
            while self.parked.len() > 4
                || (self.parked.len() > 1
                    && self
                        .parked
                        .iter()
                        .map(|(_, art)| art.as_bytes(0).map_or(0, <[u8]>::len))
                        .sum::<usize>()
                        > 24 * 1024 * 1024)
            {
                self.to_drop.push(self.parked.remove(0).1);
            }
        }
        self.started_pending = false;
        self.up_next = UpNext::Off;
        self.menu = Menu::Closed;
        self.context = None;
        self.chapters.clear();
        self.heat.clear();
        self.state.status = Status::Idle;
        self.inhibit.set(false, cx);
        cx.notify();
        at
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.source.clone() {
            let at = Some(self.state.position).filter(|p| *p > 1.0);
            self.load(&id, at, None, cx);
        }
    }

    pub fn set_previous(&mut self, video: Option<Video>, cx: &mut Context<Self>) {
        self.previous = video;
        cx.notify();
    }

    pub fn set_next(&mut self, next: Option<Video>, cx: &mut Context<Self>) {
        if self.next.as_ref().map(|v| &v.id) == next.as_ref().map(|v| &v.id) {
            self.prepare_next();
            return;
        }
        if let Some(old) = self.next_img.take() {
            self.to_drop.push(old);
        }
        self.next_req += 1;
        self.next = next;
        self.prepare_next();
        if matches!(self.up_next, UpNext::Counting { .. }) {
            self.request_next_image(cx);
        }
        cx.notify();
    }

    fn request_next_image(&self, cx: &mut Context<Self>) {
        let Some(v) = &self.next else { return };
        let req = self.next_req;
        let width = crate::images::bucket(240.0 * self.scale);
        let job = nt_core::images::image(&v.thumbnail, width);
        cx.spawn(async move |this, cx| {
            let Ok(rgba) = job.await else { return };
            let img = cx
                .background_executor()
                .spawn(async move { crate::images::to_render_image(&rgba) })
                .await;
            let _ = this.update(cx, |p, cx| {
                if p.next_req == req && matches!(p.up_next, UpNext::Counting { .. }) {
                    p.next_img = img;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn prepare_next(&self) {
        if let Some(player) = &self.player {
            player.prepare_next(
                self.next
                    .as_ref()
                    .filter(|_| {
                        self.autoplay && !self.state.looping && self.sleep != Sleep::EndOfVideo
                    })
                    .map(|v| v.id.as_str()),
            );
        }
    }

    /// Called on every player wake.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.player else { return };
        let generation = p.frame_generation();
        if self.stats {
            let frames = p.render_stats().frames;
            let t = now();
            let (since, previous) = self.stats_sample;
            if t - since >= 0.5 {
                self.render_fps = frames.saturating_sub(previous) as f64 / (t - since);
                self.stats_sample = (t, frames);
            }
        }
        let state = p.state();
        if self.state.height == 0
            && state.height > 0
            && let Some(id) = &self.source
        {
            trace("stream-metadata-observed", id);
        }
        let changed = generation != self.seen_gen || shown(&state) != shown(&self.state);
        let was = self.state.status.clone();
        self.seen_gen = generation;
        let loop_changed = self.state.looping != state.looping;
        self.state = state;
        self.test_hooks(cx);
        let gpu_frame = self.native_frame_available();
        let presentation_changed = gpu_frame != self.hardware_presenting;
        self.hardware_presenting = gpu_frame;
        if loop_changed {
            self.prepare_next();
        }
        if self.state.status == Status::Ended && was != Status::Ended {
            self.on_ended(cx);
        }
        let playing = self.state.is_playing();
        self.inhibit.set(playing, cx);
        if self.state.status == Status::Ready
            && was == Status::Loading
            && let Some(id) = &self.source
        {
            trace("ready", id);
        }
        if gpu_frame
            && presentation_changed
            && let Some(id) = &self.source
        {
            trace("gpu-frame", id);
        }
        if self.state.status == Status::Ready && was == Status::Loading && self.controls_awake {
            self.wake_controls(cx);
        }
        if changed || presentation_changed {
            cx.notify();
        }
    }

    fn on_ended(&mut self, cx: &mut Context<Self>) {
        if self.sleep == Sleep::EndOfVideo {
            // The sleep timer ends here: no autoplay.
            self.sleep = Sleep::Off;
            cx.emit(PlayerEvent::Toast("Sleep timer: playback stopped"));
            cx.notify();
            return;
        }
        if !(self.autoplay && self.next.is_some()) {
            return;
        }
        self.up_next_gen += 1;
        let generation = self.up_next_gen;
        self.up_next = UpNext::Counting {
            since: now(),
            generation,
        };
        if self.next_img.is_none() {
            self.request_next_image(cx);
        }
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

    /// Chapters and the most-replayed heatmap of the video now loaded (from its watch page).
    pub fn set_marks(
        &mut self,
        chapters: Vec<Chapter>,
        heat: Vec<HeatPoint>,
        cx: &mut Context<Self>,
    ) {
        let mut chapters = chapters;
        chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
        self.chapters = chapters;
        self.heat = heat;
        cx.notify();
    }

    /// The chapter playing at `t` seconds.
    pub fn chapter_at(&self, t: f64) -> Option<&Chapter> {
        self.chapters.iter().rev().find(|c| c.start <= t + 0.001)
    }

    /// Seek to `t` seconds (chapter rows, timestamps).
    pub fn seek_to(&mut self, t: f64, cx: &mut Context<Self>) {
        if let Some(p) = &self.player
            && !self.state.is_live
        {
            p.seek(t.max(0.0));
            if self.state.status == Status::Ended {
                self.up_next = UpNext::Cancelled;
            }
            self.wake_controls(cx);
        }
    }

    /// mpv `sub-pos` for the current lift (core §C7, as apps/iced does it). With sub-pos 100
    /// core's `sub-margin-y` (44 of 720) leaves ~6% below the caption block, so 98 rests at
    /// about 8% from the picture's bottom. Lifted, the block clears the scrubber and the bar
    /// row (48 + 16 + 12 px of air, from the player box's bottom); letterbox bars under the
    /// picture (theatre, fullscreen, odd aspect) reduce how far it has to rise.
    fn caption_percent(&self) -> f64 {
        const REST: f64 = 98.0;
        const CONTROLS: f32 = 76.0;
        let b = self.bounds.get();
        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
        let aspect = self.state.aspect().unwrap_or(16.0 / 9.0).max(0.1);
        let vh = bh.min(bw / aspect).max(1.0);
        let gap = ((bh - vh) / 2.0).max(0.0);
        let needed = f64::from(((CONTROLS - gap) / vh).max(0.0)) * 100.0;
        let lifted = (100.0 - (needed - 100.0 * 44.0 / 720.0)).min(REST);
        REST + (lifted - REST) * f64::from(self.caption_lift)
    }

    /// Move the captions with the controls: lift while they show, ease back when they hide.
    fn place_captions(&mut self, shown: bool, window: &mut Window) {
        let to = if shown { 1.0 } else { 0.0 };
        let target = self.caption_anim.map_or(self.caption_lift, |(_, t, _)| t);
        if target != to {
            self.caption_anim = Some((self.caption_lift, to, now()));
        }
        if let Some((from, to, start)) = self.caption_anim {
            // 200 ms, ease out, like YouTube's caption move.
            let t = ((now() - start) / 0.2).clamp(0.0, 1.0) as f32;
            let eased = 1.0 - (1.0 - t).powi(3);
            self.caption_lift = from + (to - from) * eased;
            if t >= 1.0 {
                self.caption_anim = None;
            } else {
                window.request_animation_frame();
            }
        }
        if self.state.selected_captions.is_none() {
            return;
        }
        let pos = self.caption_percent();
        if (pos - self.caption_pos).abs() >= 0.05 || self.caption_pos.is_nan() {
            self.caption_pos = pos;
            if let Some(p) = &self.player {
                p.set_caption_position(pos);
            }
        }
    }

    /// Test-only switches for input-free checks (`NT_TEST_CC=1` turns captions on when the
    /// tracks are known, `NT_TEST_FULLSCREEN=1` goes fullscreen once playing,
    /// `NT_TEST_CYCLE=controls` shows and hides the controls every 3.2 s).
    fn test_hooks(&mut self, cx: &mut Context<Self>) {
        let flag = |name: &str| std::env::var(name).as_deref() == Ok("1");
        if self.state.status != Status::Ready {
            return;
        }
        if !self.test_cc_done
            && flag("NT_TEST_CC")
            && !self.state.captions.is_empty()
            && self.state.selected_captions.is_none()
        {
            self.test_cc_done = true;
            trace("test-cc-on", self.source.as_deref().unwrap_or(""));
            self.toggle_captions(cx);
        }
        if !self.test_fullscreen_done && flag("NT_TEST_FULLSCREEN") {
            self.test_fullscreen_done = true;
            if !self.fullscreen {
                cx.emit(PlayerEvent::ToggleFullscreen);
            }
        }
        // `NT_TEST_SCRUB=0.6`: hold the pointer-less seek-bar hover at that fraction (tooltip,
        // storyboard and heatmap), with the controls shown.
        if self.scrub_hover.is_none()
            && let Some(f) = std::env::var("NT_TEST_SCRUB")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|f| (0.0..=1.0).contains(f))
            && self.state.duration > 0.0
        {
            self.test_controls = Some(true);
            self.scrub_hover = Some(f);
            self.request_preview(f, cx);
            cx.notify();
        }
        if !self.test_started && std::env::var("NT_TEST_CYCLE").as_deref() == Ok("controls") {
            self.test_started = true;
            self.test_controls = Some(true);
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(3200))
                        .await;
                    let alive = this.update(cx, |p, cx| {
                        let shown = !p.test_controls.unwrap_or(false);
                        p.test_controls = Some(shown);
                        trace(
                            if shown {
                                "test-controls-shown"
                            } else {
                                "test-controls-hidden"
                            },
                            "",
                        );
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Escape: close the player's own menus. Returns whether one was open.
    pub fn close_menus(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.menu != Menu::Closed || self.context.is_some();
        self.menu = Menu::Closed;
        self.context = None;
        if open {
            cx.notify();
        }
        open
    }

    pub fn close_context(&mut self, cx: &mut Context<Self>) {
        if self.context.take().is_some() {
            cx.notify();
        }
    }

    /// The open popup, for the state probe.
    pub fn popup(&self) -> Option<&'static str> {
        if self.context.is_some() {
            Some("player")
        } else if self.menu != Menu::Closed {
            Some("settings")
        } else if self.stats && self.source.is_some() {
            Some("stats")
        } else {
            None
        }
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// A frame of the current video has been drawn.
    pub fn has_picture(&self) -> bool {
        self.presented.get()
    }

    /// C: captions on with the default track, or off.
    pub fn toggle_captions(&mut self, cx: &mut Context<Self>) {
        if self.state.captions.is_empty() {
            return;
        }
        let pick = if self.state.selected_captions.is_some() {
            None
        } else {
            Some((default_track(&self.state.captions), None))
        };
        self.set_captions(pick, cx);
    }

    fn set_captions(&mut self, pick: Option<(usize, Option<String>)>, cx: &mut Context<Self>) {
        let Some(p) = &self.player else { return };
        let job = p.set_captions(pick);
        let epoch = self.source_epoch;
        cx.spawn(async move |this, cx| {
            if let Err(e) = job.await {
                eprintln!("nt-gpui: captions failed: {e}");
                let _ = this.update(cx, |p, cx| {
                    if p.source_epoch == epoch {
                        cx.emit(PlayerEvent::Toast("Subtitles aren't available right now"));
                    }
                });
            }
        })
        .detach();
        self.wake_controls(cx);
    }

    fn toggle_loop(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.state.looping;
        if enabled && self.sleep == Sleep::EndOfVideo {
            self.set_sleep(Sleep::Off, cx);
        }
        if let Some(p) = &self.player {
            p.set_loop(enabled);
        }
    }

    fn set_sleep(&mut self, sleep: Sleep, cx: &mut Context<Self>) {
        self.sleep = sleep;
        self.sleep_task = None;
        if sleep == Sleep::EndOfVideo
            && self.state.looping
            && let Some(player) = &self.player
        {
            player.set_loop(false);
        }
        self.prepare_next();
        if let Sleep::At(at, _) = sleep {
            self.sleep_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64((at - now()).max(0.0)))
                    .await;
                let _ = this.update(cx, |p, cx| {
                    p.sleep = Sleep::Off;
                    p.up_next = UpNext::Cancelled;
                    if p.source.is_some()
                        && let Some(player) = &p.player
                    {
                        player.pause();
                        cx.emit(PlayerEvent::Toast("Sleep timer: playback paused"));
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn copy(&mut self, at_time: bool, cx: &mut Context<Self>) {
        let Some(id) = self.source.clone() else {
            return;
        };
        let url = if at_time {
            format!(
                "https://www.youtube.com/watch?v={id}&t={}s",
                self.state.position.max(0.0).floor() as u64
            )
        } else {
            format!("https://www.youtube.com/watch?v={id}")
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(url));
        self.context = None;
        cx.emit(PlayerEvent::Toast("Link copied"));
        cx.notify();
    }

    pub fn play_next(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.next.clone() {
            trace("next-target", &v.id);
            cx.emit(PlayerEvent::PlayNext(Box::new(v)));
        } else if let Some(id) = &self.source {
            trace("next-unavailable", id);
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
            || self.context.is_some()
            || self.scrub_drag.is_some()
            || self.vol_drag
            || self.state.paused
            || !matches!(self.state.status, Status::Ready);
        if let Some(shown) = self.test_controls {
            return shown;
        }
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
            let f = self.scrub_fraction(x);
            self.scrub_drag = Some(f);
            self.request_preview(f, cx);
            cx.notify();
        } else if self.vol_drag {
            self.set_volume_at(x, cx);
        }
    }

    fn on_drag_end(&mut self, cx: &mut Context<Self>) {
        // Seek once, on release.
        if let Some(f) = self.scrub_drag.take() {
            self.seek_fraction(f as f64, cx);
            if self.scrub_hover.is_none() {
                self.clear_preview();
            }
        }
        self.vol_drag = false;
        cx.notify();
    }

    fn request_preview(&mut self, fraction: f32, cx: &mut Context<Self>) {
        if self.state.is_live || self.state.duration <= 0.0 {
            return;
        }
        let Some(board) = self.state.storyboard.clone() else {
            return;
        };
        let seconds = f64::from(fraction) * self.state.duration;
        let width = (160.0 * self.scale).ceil() as u32;
        let key = board.level_for_width(width).frame_at(seconds);
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key.clone());
        // Cancel the pending pointer sample if the user moves to another cell.
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(55))
                .await;
            let Ok(rgba) = board.preview(seconds, width).await else {
                return;
            };
            let image = cx
                .background_executor()
                .spawn(async move { crate::images::to_render_image(&rgba) })
                .await;
            let _ = this.update(cx, |p, cx| {
                if p.preview_key.as_ref() == Some(&key)
                    && let Some(image) = image
                {
                    if let Some(old) = p.preview_image.replace(image) {
                        p.to_drop.push(old);
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn clear_preview(&mut self) {
        self.preview_key = None;
        self.preview_task = None;
        self.to_drop.extend(self.preview_image.take());
    }

    pub fn drop_retired_images(&mut self, window: &mut Window) {
        for old in self.to_drop.drain(..) {
            let _ = window.drop_image(old);
        }
    }

    // ---- rendering ---------------------------------------------------------------------------

    fn upload(&mut self, window: &mut Window) -> bool {
        self.drop_retired_images(window);
        let Some(p) = &self.player else { return false };
        if p.frame_generation() == self.shown_gen {
            return false;
        }
        let Some(frame) = p.frame() else {
            return false;
        };
        self.shown_gen = frame.generation;
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
            .tab_index(0)
            .focus_visible(focus_ring)
            .children(target(
                cx,
                Layer::Player,
                id,
                match id {
                    "play" => "Play / pause",
                    "next" => "Next",
                    "settings" => "Settings",
                    "theatre" => "Theatre mode",
                    "fullscreen" => "Fullscreen",
                    "cc" => "Subtitles",
                    _ => id,
                },
            ))
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
        let played = if live { 1.0 } else { frac };
        let bounds = self.scrub_bounds.clone();
        let tip = self.scrub_drag.or(self.scrub_hover);
        let tip_el = tip.filter(|_| d > 0.0 && !live).map(|f| {
            let track_w = f32::from(bounds.get().size.width).max(1.0);
            let tip_w = 200.0_f32.min(track_w);
            let left = (f * track_w - tip_w / 2.0).clamp(0.0, track_w - tip_w);
            let chapter = self
                .chapter_at(f as f64 * d)
                .map(|c| c.title.clone())
                .filter(|_| self.chapters.len() > 1);
            div()
                .absolute()
                .bottom(px(18.0))
                .left(px(left))
                .w(px(tip_w))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .when_some(self.preview_image.clone(), |d, preview| {
                    d.child(
                        img(preview)
                            .w(px(160.0))
                            .h(px(90.0))
                            .rounded(px(6.0))
                            .object_fit(ObjectFit::Contain)
                            .bg(black()),
                    )
                })
                .when_some(chapter, |el, t| {
                    el.child(
                        line(t, 13.0, 18.0, W500, text())
                            .max_w(px(200.0))
                            .px(px(6.0))
                            .rounded(px(4.0))
                            .bg(badge_bg()),
                    )
                })
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(badge_bg())
                        .child(txt(format::clock(f as f64 * d), 12.0, 18.0, W500, text())),
                )
        });
        // Chapter segments with 2 px gaps; one segment without chapters.
        let mut breaks = vec![0.0];
        if d > 0.0 && !live {
            breaks.extend(
                self.chapters
                    .iter()
                    .filter_map(|c| (c.start > 0.0 && c.start < d).then_some((c.start / d) as f32)),
            );
        }
        breaks.push(1.0);
        breaks.dedup();
        let segs = breaks.windows(2);
        let last = segs.len() - 1;
        let mut track = div()
            .relative()
            .w_full()
            .h(px(3.0))
            .mb(px(2.0))
            .group_hover("scrub", |s| s.h(px(5.0)).mb(px(1.0)));
        for (i, segment) in segs.enumerate() {
            let a = segment[0];
            let b = segment[1];
            let part = |x: f32| ((x - a) / (b - a)).clamp(0.0, 1.0);
            track = track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(a))
                    .w(gpui::relative(b - a))
                    .when(i != last, |d| d.pr(px(2.0)))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .bg(fill_hover())
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui::relative(part(buffered)))
                                    .bg(gpui::rgba(0xffffff66)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui::relative(part(played)))
                                    .bg(red()),
                            ),
                    ),
            );
        }
        track = track.child(
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
        );
        // Most replayed: a soft curve above the bar while the pointer is on it.
        let heat = (tip.is_some() && !self.heat.is_empty() && d > 0.0).then(|| {
            let points: Vec<(f32, f32)> = self
                .heat
                .iter()
                .map(|h| {
                    (
                        ((h.start + h.duration / 2.0) / d).clamp(0.0, 1.0) as f32,
                        h.intensity.clamp(0.0, 1.0) as f32,
                    )
                })
                .collect();
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let (x0, w) = (f32::from(b.origin.x), f32::from(b.size.width));
                    let (top, h) = (f32::from(b.origin.y), f32::from(b.size.height));
                    let bottom = top + h;
                    let mut path = gpui::PathBuilder::fill();
                    path.move_to(gpui::point(px(x0), px(bottom)));
                    for (fx, fy) in &points {
                        path.line_to(gpui::point(px(x0 + fx * w), px(bottom - fy * h)));
                    }
                    path.line_to(gpui::point(px(x0 + w), px(bottom)));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, gpui::rgba(0xffffff40));
                    }
                },
            )
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(6.0))
            .h(px(40.0))
        });
        div()
            .id("scrub")
            .when(d > 0.0 && !live, |d| {
                d.children(target(cx, Layer::Player, "scrubber", "Seek"))
            })
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
                    let f = this.scrub_fraction(f32::from(ev.position.x));
                    this.scrub_drag = Some(f);
                    this.request_preview(f, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let f = this.scrub_fraction(f32::from(ev.position.x));
                if this.scrub_hover != Some(f) {
                    this.scrub_hover = Some(f);
                    if this.scrub_drag.is_none() {
                        this.request_preview(f, cx);
                    }
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.scrub_hover.take().is_some() {
                    if this.scrub_drag.is_none() {
                        this.clear_preview();
                    }
                    cx.notify();
                }
            }))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .children(heat)
            .child(track)
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
        let mut panel = div()
            .id("player-menu")
            .occlude()
            .children(target(cx, Layer::Settings, "_panel", ""))
            .absolute()
            .right(px(12.0))
            .bottom(px(60.0))
            .w(px(260.0))
            .max_h(px(
                (f32::from(self.bounds.get().size.height) - 72.0).max(120.0)
            ))
            .overflow_y_scroll()
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(gpui::rgba(0x282828ee))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match self.menu {
            Menu::Main => {
                if !s.captions.is_empty() {
                    let value = match &s.selected_captions {
                        Some((i, tr)) => {
                            let base = s
                                .captions
                                .get(*i)
                                .map(track_label)
                                .unwrap_or_else(|| "On".to_string());
                            match tr.as_ref().and_then(|code| language_label(s, code)) {
                                Some(l) => format!("{base} >> {l}"),
                                None => base,
                            }
                        }
                        None => "Off".to_string(),
                    };
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            "m-cc",
                            Some("closed_caption"),
                            format!("Subtitles/CC ({})", s.captions.len()).into(),
                            Some(value.into()),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = Menu::Captions;
                            cx.notify();
                        })),
                    );
                }
                if !s.audio_tracks.is_empty() {
                    let label = s
                        .audio_tracks
                        .iter()
                        .find(|t| Some(&t.id) == s.audio_track.as_ref())
                        .map_or("Default", |t| t.label.as_str());
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            "m-audio",
                            Some("volume_up"),
                            "Audio track".into(),
                            Some(label.to_owned().into()),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = Menu::Audio;
                            cx.notify();
                        })),
                    );
                }
                for (id, name, label, active) in [
                    ("m-volume", "volume_up", "Stable volume", s.normalize),
                    ("m-loop", "repeat", "Loop", s.looping),
                ] {
                    if id == "m-volume" && !s.normalization_supported {
                        continue;
                    }
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            id,
                            Some(name),
                            label.into(),
                            None,
                            false,
                        )
                        .child(switch(active))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if id == "m-loop" {
                                this.toggle_loop(cx);
                            } else if let Some(p) = &this.player {
                                p.set_normalize(!this.state.normalize);
                            }
                        })),
                    );
                }
                let sleep = match self.sleep {
                    Sleep::Off => "Off".to_string(),
                    Sleep::EndOfVideo => "End of video".to_string(),
                    Sleep::At(at, _) => {
                        format!("{} min", ((at - now()) / 60.0).ceil().max(1.0) as i64)
                    }
                };
                panel = panel.child(
                    menu_row(
                        cx,
                        Layer::Settings,
                        "m-sleep",
                        Some("timer"),
                        "Sleep timer".into(),
                        Some(sleep.into()),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = Menu::Sleep;
                        cx.notify();
                    })),
                );
                if !s.is_live {
                    let speed = if (s.speed - 1.0).abs() < 0.01 || s.speed == 0.0 {
                        "Normal".to_string()
                    } else {
                        format!("{}", s.speed)
                    };
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
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
                    menu_row(
                        cx,
                        Layer::Settings,
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
                        menu_row(
                            cx,
                            Layer::Settings,
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
                        menu_row(
                            cx,
                            Layer::Settings,
                            QUALITY_IDS.get(i).copied().unwrap_or("q"),
                            None,
                            format!("{h}p").into(),
                            None,
                            chosen == Some(h),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = &this.player {
                                p.set_quality(Some(h));
                                this.prepare_next();
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
                    menu_row(
                        cx,
                        Layer::Settings,
                        "q-auto",
                        None,
                        auto.into(),
                        None,
                        chosen.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(p) = &this.player {
                            p.set_quality(None);
                            this.prepare_next();
                        }
                        this.menu = Menu::Closed;
                        cx.notify();
                    })),
                );
            }
            Menu::Captions => {
                panel = panel.child(self.menu_header("Subtitles/CC", cx));
                let sel = s.selected_captions.clone();
                panel = panel.child(
                    menu_row(
                        cx,
                        Layer::Settings,
                        "cc-off",
                        None,
                        "Off".into(),
                        None,
                        sel.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_captions(None, cx);
                        this.menu = Menu::Closed;
                        cx.notify();
                    })),
                );
                for (i, t) in s.captions.iter().enumerate() {
                    let on = sel.as_ref().is_some_and(|(j, tr)| *j == i && tr.is_none());
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            ("cc", i),
                            None,
                            track_label(t).into(),
                            None,
                            on,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_captions(Some((i, None)), cx);
                            this.menu = Menu::Closed;
                            cx.notify();
                        })),
                    );
                }
                let translatable = sel
                    .as_ref()
                    .and_then(|(i, _)| s.captions.get(*i))
                    .is_some_and(|t| t.translatable)
                    && !s.caption_languages.is_empty();
                if translatable {
                    let current = sel
                        .as_ref()
                        .and_then(|(_, tr)| tr.as_deref())
                        .and_then(|code| language_label(s, code))
                        .unwrap_or_else(|| "Off".to_string());
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            "cc-translate",
                            None,
                            "Auto-translate".into(),
                            Some(current.into()),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = Menu::Translate;
                            cx.notify();
                        })),
                    );
                }
            }
            Menu::Translate => {
                panel = panel.child(self.menu_header("Auto-translate", cx));
                let sel = s.selected_captions.clone();
                if let Some((track, tr)) = sel {
                    for (i, l) in s.caption_languages.iter().enumerate() {
                        let code = l.code.clone();
                        let on = tr.as_deref() == Some(code.as_str());
                        panel = panel.child(
                            menu_row(
                                cx,
                                Layer::Settings,
                                ("tr", i),
                                None,
                                l.label.clone().into(),
                                None,
                                on,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.set_captions(Some((track, Some(code.clone()))), cx);
                                    this.menu = Menu::Closed;
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
            }
            Menu::Sleep => {
                panel = panel.child(self.menu_header("Sleep timer", cx));
                let mut opts: Vec<(Sleep, String)> = vec![(Sleep::Off, "Off".into())];
                for m in SLEEP_MINUTES {
                    opts.push((Sleep::At(0.0, m), format!("{m} minutes")));
                }
                opts.push((Sleep::EndOfVideo, "End of video".into()));
                for (i, (opt, label)) in opts.into_iter().enumerate() {
                    let on = match (self.sleep, opt) {
                        (Sleep::At(_, a), Sleep::At(_, b)) => a == b,
                        (a, b) => a == b,
                    };
                    panel = panel.child(
                        menu_row(cx, Layer::Settings, ("sl", i), None, label.into(), None, on)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let pick = match opt {
                                    Sleep::At(_, m) => Sleep::At(now() + m as f64 * 60.0, m),
                                    other => other,
                                };
                                this.set_sleep(pick, cx);
                                this.menu = Menu::Closed;
                                cx.notify();
                            })),
                    );
                }
            }
            Menu::Audio => {
                panel = panel.child(self.menu_header("Audio track", cx));
                for (i, t) in s.audio_tracks.iter().enumerate() {
                    let id = t.id.clone();
                    let label = if t.auto_dubbed && !t.label.contains("auto-dubbed") {
                        format!("{} (auto-dubbed)", t.label)
                    } else {
                        t.label.clone()
                    };
                    panel = panel.child(
                        menu_row(
                            cx,
                            Layer::Settings,
                            ("audio", i),
                            None,
                            label.into(),
                            None,
                            s.audio_track.as_ref() == Some(&id),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(p) = &this.player else { return };
                            let job = p.set_audio_track(&id);
                            let epoch = this.source_epoch;
                            cx.spawn(async move |this, cx| {
                                if let Err(e) = job.await {
                                    eprintln!("nt-gpui: audio track: {e}");
                                    let _ = this.update(cx, |p, cx| {
                                        if p.source_epoch == epoch {
                                            cx.emit(PlayerEvent::Toast(
                                                "Couldn't change audio track",
                                            ));
                                        }
                                    });
                                }
                            })
                            .detach();
                            this.close_menus(cx);
                        })),
                    );
                }
            }
            Menu::Closed => {}
        }
        Some(panel)
    }

    fn menu_header(&self, title: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("menu-back")
            .tab_index(0)
            .focus_visible(focus_ring)
            .children(target(cx, Layer::Settings, "menu_item", "Back"))
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
        // "• Intro ›": the chapter playing now.
        let chapter = (self.chapters.len() > 1 && !s.is_live)
            .then(|| {
                let t = self
                    .scrub_drag
                    .map_or(s.position, |f| f as f64 * s.duration);
                self.chapter_at(t).map(|c| c.title.clone())
            })
            .flatten()
            .map(|title| {
                div()
                    .id("current-chapter")
                    .ml(px(8.0))
                    .min_w(px(0.0))
                    .flex_shrink(1.0)
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .tab_index(0)
                    .focus_visible(focus_ring)
                    .hover(|s| s.bg(fill()))
                    .children(target(cx, Layer::Player, "chapter_list", title.as_str()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PlayerEvent::ShowChapters)))
                    .child(txt("•", 13.0, 18.0, W400, c(0xeeeeee)).mr(px(6.0)))
                    .child(line(title, 13.0, 18.0, W400, c(0xeeeeee)).max_w(px(260.0)))
                    .child(icon("chevron_right", 20.0, c(0xeeeeee)))
            });
        let cc_on = s.selected_captions.is_some();
        let cc = (!s.captions.is_empty()).then(|| {
            div()
                .relative()
                .child(self.icon_button(
                    "cc",
                    if cc_on {
                        "closed_caption_fill1"
                    } else {
                        "closed_caption"
                    },
                    cx,
                    |this, _, cx| this.toggle_captions(cx),
                ))
                .when(cc_on, |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom(px(4.0))
                            .left(px(10.0))
                            .w(px(20.0))
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(red()),
                    )
                })
        });
        div()
            .h(px(48.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .when(self.previous.is_some(), |d| {
                d.child(
                    self.icon_button("previous", "skip_previous", cx, |this, _, cx| {
                        if let Some(v) = this.previous.clone() {
                            cx.emit(PlayerEvent::PlayPrevious(Box::new(v)));
                        }
                    }),
                )
            })
            .child(self.icon_button("play", play_icon, cx, |this, _, cx| this.toggle_pause(cx)))
            .when(self.next.is_some(), |d| {
                d.child(self.icon_button("next", "skip_next", cx, |this, _, cx| this.play_next(cx)))
            })
            .child(self.volume(cx))
            .child(time)
            .children(chapter)
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
                        this.prepare_next();
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
            .children(cc)
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

    /// The right-click menu: opens at the pointer and stays inside the player.
    fn context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (x, y) = self.context?;
        let b = self.bounds.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let width = 280.0_f32.min((w - 16.0).max(1.0));
        let mh = 16.0 + 4.0 * 40.0;
        let x = if x + width > w - 8.0 { x - width } else { x };
        let y = if y + mh > h - 8.0 { y - mh } else { y };
        let x = x.clamp(8.0, (w - width - 8.0).max(8.0));
        let y = y.clamp(8.0, (h - mh - 8.0).max(8.0));
        let item = |id: &'static str, name: &'static str, label: &'static str| {
            div()
                .id(id)
                .tab_index(0)
                .focus_visible(focus_ring)
                .children(target(cx, Layer::PlayerMenu, "menu_item", label))
                .h(px(40.0))
                .px(px(16.0))
                .flex()
                .items_center()
                .gap(px(16.0))
                .cursor_pointer()
                .hover(|s| s.bg(fill()))
                .child(icon(name, 24.0, text()))
                .child(txt(label, 14.0, 20.0, W500, text()))
        };
        let panel = div()
            .id("player-context")
            .children(target(cx, Layer::PlayerMenu, "_panel", ""))
            .occlude()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(gpui::rgba(0x282828ee))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                item("ctx-url", "link", "Copy video URL")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(false, cx))),
            )
            .child(
                item("ctx-url-time", "link", "Copy video URL at current time")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(true, cx))),
            )
            .child(
                item("ctx-loop", "repeat", "Loop")
                    .when(self.state.looping, |d| {
                        d.child(div().flex_1()).child(icon("check", 24.0, text()))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_loop(cx);
                        this.close_menus(cx);
                    })),
            )
            .child(
                item("ctx-stats", "info", "Stats for nerds").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.stats = true;
                        this.stats_sample = (
                            now(),
                            this.player.as_ref().map_or(0, |p| p.render_stats().frames),
                        );
                        this.render_fps = 0.0;
                        this.context = None;
                        cx.notify();
                    },
                )),
            );
        Some(
            div()
                .id("player-context-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .tab_group()
                .tab_index(0)
                .tab_stop(false)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|p, _, _, cx| {
                        cx.stop_propagation();
                        p.close_menus(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|p, _, _, cx| {
                        cx.stop_propagation();
                        p.close_menus(cx);
                    }),
                )
                .child(panel),
        )
    }

    fn stats_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.stats {
            return None;
        }
        let id = self.source.clone()?;
        let s = &self.state;
        let rs = self
            .player
            .as_ref()
            .map(|p| p.render_stats())
            .unwrap_or_default();
        let b = self.bounds.get();
        let res = match s.video_size {
            Some((w, h)) => format!("{w}x{h}"),
            None => "-".to_string(),
        };
        let avg = if rs.frames > 0 {
            rs.total_render_ms / rs.frames as f64
        } else {
            0.0
        };
        let caption = match &s.selected_captions {
            Some((i, tr)) => {
                let base = s.captions.get(*i).map(track_label).unwrap_or_default();
                match tr {
                    Some(code) => format!("{base} -> {code}"),
                    None => base,
                }
            }
            None => "Off".to_string(),
        };
        let status = match &s.status {
            Status::Idle => "idle",
            Status::Loading => "loading",
            Status::Ready if s.buffering => "buffering",
            Status::Ready if s.paused => "paused",
            Status::Ready => "playing",
            Status::Ended => "ended",
            Status::Failed(_) => "failed",
        };
        let rows: Vec<(&'static str, String)> = vec![
            ("Video ID", id),
            (
                "Viewport / Frames",
                format!(
                    "{:.0}x{:.0}*{:.2} / {} frames",
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                    self.scale,
                    rs.frames
                ),
            ),
            (
                "Current / Optimal Res",
                format!(
                    "{res} / {}",
                    s.heights
                        .iter()
                        .max()
                        .map_or("-".to_string(), |h| format!("{h}p"))
                ),
            ),
            (
                "Render FPS",
                format!("{:.1}", if s.is_playing() { self.render_fps } else { 0.0 }),
            ),
            ("Codecs", "Unavailable".into()),
            (
                "Volume",
                format!("{:.0}%{}", s.volume, if s.muted { " (muted)" } else { "" }),
            ),
            (
                "Speed",
                format!("{:.2}x", if s.speed > 0.0 { s.speed } else { 1.0 }),
            ),
            ("Buffer Health", format!("{:.2} s", s.buffered_ahead)),
            (
                "Render",
                format!("{avg:.2} ms avg / {:.2} ms max", rs.max_render_ms),
            ),
            ("Captions", caption),
            ("Status", status.to_string()),
        ];
        let mut panel = div()
            .id("stats")
            .children(target(cx, Layer::Stats, "_panel", ""))
            .occlude()
            .absolute()
            .top(px(12.0))
            .left(px(12.0))
            .w(px(420.0))
            .max_w(gpui::relative(0.95))
            .p(px(8.0))
            .pr(px(32.0))
            .bg(gpui::rgba(0x000000b3))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("stats-close")
                    .tab_index(0)
                    .focus_visible(focus_ring)
                    .children(target(cx, Layer::Stats, "close", "Close stats"))
                    .absolute()
                    .top(px(4.0))
                    .right(px(4.0))
                    .size(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(fill()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stats = false;
                        cx.notify();
                    }))
                    .child(icon("close", 18.0, text())),
            );
        for (k, v) in rows {
            panel = panel.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        txt(k, 11.0, 16.0, W700, text())
                            .w(gpui::relative(0.36))
                            .flex_none()
                            .text_right(),
                    )
                    .child(line(v, 11.0, 16.0, W400, text()).flex_1().min_w(px(0.0))),
            );
        }
        Some(panel)
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

/// A row of the player's settings menu: an icon (or a check slot), a label, and a value with ›.
fn menu_row(
    cx: &gpui::App,
    layer: Layer,
    id: impl Into<ElementId>,
    left: Option<&'static str>,
    label: SharedString,
    value: Option<SharedString>,
    check: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .tab_index(0)
        .focus_visible(focus_ring)
        .children(target(cx, layer, "menu_item", label.clone()))
        .h(px(40.0))
        .flex_none()
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
            d.child(line(v, 14.0, 20.0, W400, text()).max_w(px(120.0)))
                .child(icon("chevron_right", 24.0, text()))
        })
}

/// YouTube's small on/off switch in the settings menu.
fn switch(on: bool) -> gpui::Div {
    div()
        .relative()
        .w(px(36.0))
        .h(px(14.0))
        .flex_none()
        .rounded_full()
        .bg(if on { red().opacity(0.6) } else { fill_hover() })
        .child(
            div()
                .absolute()
                .top(px(-3.0))
                .left(px(if on { 16.0 } else { 0.0 }))
                .size(px(20.0))
                .rounded_full()
                .bg(if on { red() } else { c(0xf1f1f1) }),
        )
}

/// "English (auto-generated)".
fn track_label(t: &nt_core::CaptionTrack) -> String {
    if t.auto_generated && !t.label.to_lowercase().contains("auto") {
        format!("{} (auto-generated)", t.label)
    } else {
        t.label.clone()
    }
}

fn language_label(s: &PlaybackState, code: &str) -> Option<String> {
    s.caption_languages
        .iter()
        .find(|l| l.code == code)
        .map(|l| l.label.clone())
}

/// YouTube's default: a manual English track, then any English, then the first manual track.
fn default_track(tracks: &[nt_core::CaptionTrack]) -> usize {
    let en = |t: &nt_core::CaptionTrack| t.language_code.starts_with("en");
    tracks
        .iter()
        .position(|t| en(t) && !t.auto_generated)
        .or_else(|| tracks.iter().position(en))
        .or_else(|| tracks.iter().position(|t| !t.auto_generated))
        .unwrap_or(0)
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
        probe::begin(
            cx,
            &[
                Layer::Player,
                Layer::PlayerMenu,
                Layer::Settings,
                Layer::Stats,
            ],
        );
        self.scale = window.scale_factor();
        let new_picture = self.upload(window) || self.native_paint;
        if std::mem::take(&mut self.cursor_hide) {
            // Not `cx.defer_in`: that runs inside this view's update, and dispatching a key
            // draws the window first when it is dirty, which renders this view again (the
            // "already being updated" panic). `Window::defer` holds no entity.
            window.defer(cx, hide_cursor);
        }
        let mark = new_picture && std::mem::take(&mut self.started_pending);
        if mark {
            self.poster_task = None;
            if let Some((_, poster)) = self.poster.take() {
                let _ = window.drop_image(poster);
            }
            self.poster_fallback = None;
        }
        let s = &self.state;
        let failed = self.start_error.is_some() || matches!(s.status, Status::Failed(_));
        let waiting_for_frame = self.started_pending && !new_picture;
        let loading = !failed
            && self.source.is_some()
            && (waiting_for_frame || matches!(s.status, Status::Loading) || s.buffering);
        let show = self.controls_visible() && self.source.is_some() && !failed;
        self.place_captions(show, window);
        let rounded = !(self.theatre || self.fullscreen);
        let me = cx.entity().downgrade();
        let presented = self.presented.clone();
        let bounds = self.bounds.clone();
        let previous_target = self.target;
        let scale = window.scale_factor();
        let trace_id = (mark && tracing()).then(|| self.source.clone()).flatten();
        let dragging = self.scrub_drag.is_some() || self.vol_drag;
        // Bounds, target size, drag tracking outside the player, and the playback mark.
        let hook = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, cx| {
                let w = (f32::from(b.size.width) * scale).round() as u32;
                let h = (f32::from(b.size.height) * scale).round() as u32;
                if (w, h) != previous_target && w > 0 && h > 0 {
                    let _ = me.update(cx, |p, _| p.set_target((w, h)));
                }
                if mark {
                    if let Some(id) = &trace_id {
                        trace("paint", id);
                    }
                    presented.set(true);
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
            .tab_group()
            .tab_index(3)
            .tab_stop(false)
            .when(!failed, |d| {
                d.children(target(cx, Layer::Player, "player", "Player"))
            })
            .relative()
            .size_full()
            .when(!self.native_paint, |d| d.bg(black()))
            .when(rounded, |d| d.rounded(px(12.0)))
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                // Track the seek-bar hover from the player's own moves too: when the pointer
                // lands on the bar in the same move that wakes the controls, the bar's own
                // listener isn't in that frame yet and the move is missed.
                this.wake_controls(cx);
                let inside = this.scrub_bounds.get().contains(&ev.position);
                if inside {
                    let f = this.scrub_fraction(f32::from(ev.position.x));
                    if this.scrub_hover != Some(f) {
                        this.scrub_hover = Some(f);
                        if this.scrub_drag.is_none() {
                            this.request_preview(f, cx);
                        }
                        cx.notify();
                    }
                } else if this.scrub_drag.is_none() && this.scrub_hover.take().is_some() {
                    this.clear_preview();
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                if *hovered {
                    this.wake_controls(cx);
                } else {
                    if this.scrub_hover.take().is_some() && this.scrub_drag.is_none() {
                        this.clear_preview();
                    }
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if this.menu != Menu::Closed || this.context.is_some() {
                        this.menu = Menu::Closed;
                        this.context = None;
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.source.is_none() {
                        return;
                    }
                    let b = this.bounds.get();
                    this.menu = Menu::Closed;
                    this.context = Some((
                        f32::from(ev.position.x - b.origin.x),
                        f32::from(ev.position.y - b.origin.y),
                    ));
                    cx.notify();
                }),
            )
            .child(hook);
        let poster = self
            .poster
            .as_ref()
            .filter(|(id, _)| self.source.as_deref() == Some(id.as_str()))
            .map(|(_, img)| img.clone());
        if !self.native_paint
            && let Some(frame) = self
                .image
                .clone()
                .or(poster)
                .or_else(|| self.poster_fallback.clone())
        {
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
        root = root.children(self.stats_panel(cx));
        root = root.children(self.up_next_card(cx));
        root = root.children(self.context_menu(cx));
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

impl PlayerView {
    pub fn failed(&self) -> bool {
        self.start_error.is_some() || matches!(self.state.status, Status::Failed(_))
    }

    pub fn probe_state(&self) -> Option<serde_json::Value> {
        let s = &self.state;
        let status = match &s.status {
            _ if self.failed() => "failed",
            Status::Idle => "idle",
            Status::Loading => "loading",
            Status::Ready => "ready",
            Status::Ended => "ended",
            Status::Failed(_) => "failed",
        };
        Some(serde_json::json!({
            "id": self.source()?,
            "status": status,
            "position": s.position,
            "duration": s.duration,
            "paused": s.paused,
            "muted": s.muted,
            "first_frame": self.has_picture(),
        }))
    }
}
