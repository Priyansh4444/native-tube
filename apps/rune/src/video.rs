//! Playback through `nt_core::Player` (libmpv's software renderer, RGBA).
//!
//! How a frame reaches the screen: mpv renders into one of core's pooled buffers on the
//! player's render thread, then calls our waker *on that thread*. The waker compares
//! `frame_generation()` with the last one it uploaded and, only when it changed, copies the
//! frame into a `rune::Pixels` (`Pixels::set`, one memcpy, still off the UI thread). `set` wakes
//! only the window showing the image; Rune's next frame copies it into its image page (a
//! same-size frame reuses its slot) and uploads it to the GPU. No view commit happens for a
//! video frame. Position, buffered and the clock are `Live` values the waker's coalesced
//! `update_if` sets; only discrete changes (play/pause, loading, quality, speed, errors) re-run
//! the view.
//!
//! The UI thread never calls into mpv. Every player command (start the player, load, stop,
//! seek, volume, speed, quality, target size) is queued to one command thread, in order; a
//! burst of loads and stops (clicking through related videos) keeps only the last. The waker
//! publishes a picture only when the player's source is the video the UI asked for, so a
//! frame of the previous video can never show on a new watch page.

use crate::app::handle;
use nt_core::{PixelFormat, PlaybackState, Player, Status, format, library};
use rune::{Live, LiveText, Pixels, Tween};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The player, made on the first video. A failed start leaves it empty, so the next video
/// (or Retry) tries again rather than disabling playback for the session.
static PLAYER: Mutex<Option<Arc<Player>>> = Mutex::new(None);
static PIXELS: OnceLock<Pixels> = OnceLock::new();
static LAST_GEN: AtomicU64 = AtomicU64::new(0);
/// Set by `play`; the first picture after it raises `FIRST_PICTURE`.
static WANT_FIRST: AtomicBool = AtomicBool::new(false);
/// The first picture of the current video was handed to Rune. `poll` (on the UI thread, at the
/// start of the frame that draws it) turns it into the `playback_started` mark.
static FIRST_PICTURE: AtomicBool = AtomicBool::new(false);
/// An `update_if` is already queued for the UI thread.
static SYNC_QUEUED: AtomicBool = AtomicBool::new(false);

/// The video id the UI wants on screen; the waker drops pictures of any other.
static WANT_ID: Mutex<String> = Mutex::new(String::new());

type Job = Box<dyn FnOnce(&Player) + Send>;

enum Cmd {
    Load { id: String, target: (u32, u32) },
    Stop,
    Do(Job),
}

impl Cmd {
    fn is_load_or_stop(&self) -> bool {
        matches!(self, Cmd::Load { .. } | Cmd::Stop)
    }
}

static QUEUE: OnceLock<Mutex<mpsc::Sender<Cmd>>> = OnceLock::new();

fn send(cmd: Cmd) {
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let started = std::thread::Builder::new()
            .name("nt-player-cmd".into())
            .spawn(move || command_loop(rx));
        if let Err(e) = started {
            eprintln!("nt-rune: can't start the player thread: {e}");
        }
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(cmd);
}

/// The command thread: runs what the UI queued, in order, keeping only the last of a burst of
/// loads and stops.
fn command_loop(rx: mpsc::Receiver<Cmd>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        let last = batch.iter().rposition(Cmd::is_load_or_stop);
        for (i, cmd) in batch.into_iter().enumerate() {
            if cmd.is_load_or_stop() && Some(i) != last {
                continue;
            }
            run(cmd);
        }
    }
}

fn run(cmd: Cmd) {
    match cmd {
        Cmd::Load { id, target } => match player() {
            Ok(p) => {
                if let Err(e) = p.load(&id) {
                    report(&id, e.to_string());
                    return;
                }
                p.set_volume(library().volume());
                if target.0 > 0 && target.1 > 0 {
                    p.set_target_size(target.0, target.1);
                }
            }
            Err(e) => report(&id, e),
        },
        Cmd::Stop => {
            if let Some(p) = current() {
                p.stop();
            }
        }
        Cmd::Do(f) => {
            if let Some(p) = current() {
                f(&p);
            }
        }
    }
}

/// A load or the player's start failed: the watch page shows it with a Retry.
fn report(id: &str, msg: String) {
    eprintln!("nt-rune: playback of {id} failed: {msg}");
    let id = id.to_string();
    if let Some(h) = handle() {
        h.update(move |s, _| {
            if s.video.id.as_deref() == Some(id.as_str()) {
                s.video.error = Some(msg);
            }
        });
    }
}

fn want(id: &str) {
    let mut w = WANT_ID.lock().unwrap_or_else(|e| e.into_inner());
    w.clear();
    w.push_str(id);
}

fn wanted(source: Option<&str>) -> bool {
    let w = WANT_ID.lock().unwrap_or_else(|e| e.into_inner());
    !w.is_empty() && source == Some(w.as_str())
}

pub fn pixels() -> &'static Pixels {
    PIXELS.get_or_init(Pixels::new)
}

fn current() -> Option<Arc<Player>> {
    PLAYER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn waker() {
    let Some(p) = current() else { return };
    let g = p.frame_generation();
    if g != LAST_GEN.load(Ordering::Acquire) && wanted(p.state().source.as_deref()) {
        LAST_GEN.store(g, Ordering::Release);
        if let Some(f) = p.frame() {
            pixels().set(f.width, f.height, f.pixels().to_vec());
            if WANT_FIRST.swap(false, Ordering::AcqRel) {
                FIRST_PICTURE.store(true, Ordering::Release);
            }
        }
    }
    if !SYNC_QUEUED.swap(true, Ordering::AcqRel)
        && let Some(h) = handle()
    {
        h.update_if(|s, tx| {
            SYNC_QUEUED.store(false, Ordering::Release);
            s.player_changed(tx)
        });
    }
}

/// The player, starting it if there is none (on the command thread only: `Player::new` loads
/// libmpv and starts MPRIS).
fn player() -> Result<Arc<Player>, String> {
    let mut slot = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = slot.as_ref() {
        return Ok(p.clone());
    }
    let p = Arc::new(Player::new(PixelFormat::Rgba, waker).map_err(|e| e.to_string())?);
    *slot = Some(p.clone());
    Ok(p)
}

/// Whether the first picture of the current video reached Rune since the last call.
pub fn take_first_picture() -> bool {
    FIRST_PICTURE.swap(false, Ordering::AcqRel)
}

pub const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

pub fn speed_label(s: f64) -> String {
    if (s - 1.0).abs() < 0.01 {
        "Normal".into()
    } else {
        format!("{s}")
    }
}

/// What the view shows about playback. Only these trigger a view run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub status: Status,
    pub paused: bool,
    pub buffering: bool,
    pub muted: bool,
    pub volume: i32,
    pub has_picture: bool,
    /// Speed in hundredths.
    pub speed: i32,
    pub is_live: bool,
    pub height: u32,
    pub heights: Vec<u32>,
    pub quality: Option<u32>,
}

impl Shown {
    pub fn speed(&self) -> f64 {
        if self.speed == 0 {
            1.0
        } else {
            self.speed as f64 / 100.0
        }
    }

    pub fn playing(&self) -> bool {
        self.status == Status::Ready && !self.paused
    }

    /// Loading the stream, or stalled (which includes the core recovering an expired link).
    pub fn waiting(&self) -> bool {
        matches!(self.status, Status::Loading) || self.buffering
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Menu {
    #[default]
    Closed,
    Main,
    Speed,
    Quality,
}

pub struct VideoState {
    pub shown: Shown,
    /// The player couldn't start (`Player::new` or `load` failed).
    pub error: Option<String>,
    /// The video loaded, for Retry.
    pub id: Option<String>,
    /// Scrubber fill and buffered-ahead, `[x, y, w, h]` in the track.
    pub progress: Live<[f32; 4]>,
    pub buffered: Live<[f32; 4]>,
    /// The scrubber knob and the hover tooltip's offset and text.
    pub knob: Live<[f32; 4]>,
    pub tip: Live<[f32; 2]>,
    pub tip_text: LiveText,
    pub clock: LiveText,
    /// The scrubber's width in points.
    pub seek_w: f32,
    pub duration: f64,
    pub position: f64,
    resume_at: Option<f64>,
    box_px: (u32, u32),
    pub active: bool,
    /// A scrubber drag in progress: the target fraction (one seek on release).
    pub drag: Option<f32>,
    pub scrub_hover: bool,
    pub vol_hover: bool,
    pub menu: Menu,
    /// Controls: the pointer is over the player, and when it last moved.
    pub pointer_in: bool,
    pub last_move: f64,
    pub controls: bool,
    pub hide_due: Option<f64>,
    /// The click bezel: which icon (true = play) and its fade.
    pub bezel: Option<(bool, Tween<f32>)>,
    /// "Up next": when the countdown ends, and its label.
    pub up_next: Option<f64>,
    pub up_next_text: LiveText,
    /// The countdown was cancelled for this video's end.
    pub up_next_cancelled: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        VideoState {
            shown: Shown::default(),
            error: None,
            id: None,
            progress: Live::new([0.0, 0.0, 0.0, 3.0]),
            buffered: Live::new([0.0, 0.0, 0.0, 3.0]),
            knob: Live::new([0.0, 0.0, 13.0, 13.0]),
            tip: Live::new([0.0, 0.0]),
            tip_text: LiveText::new("0:00"),
            clock: LiveText::new("0:00 / 0:00"),
            seek_w: 100.0,
            duration: 0.0,
            position: 0.0,
            resume_at: None,
            box_px: (0, 0),
            active: false,
            drag: None,
            scrub_hover: false,
            vol_hover: false,
            menu: Menu::Closed,
            pointer_in: false,
            last_move: 0.0,
            controls: true,
            hide_due: None,
            bezel: None,
            up_next: None,
            up_next_text: LiveText::new("Up next in 5"),
            up_next_cancelled: false,
        }
    }
}

pub const CLOCK_PLACEHOLDER: &str = "00:00:00 / 00:00:00";
pub const CONTROLS_HIDE: f64 = 3.0;

impl VideoState {
    /// Loads and plays `id`, from `resume_at` when returning to it. Never shows the previous
    /// video's picture: the image is cleared first, and core publishes frames of the new video
    /// only.
    pub fn play(&mut self, id: &str, resume_at: Option<f64>) {
        self.id = Some(id.to_string());
        self.active = true;
        self.resume_at = resume_at.filter(|t| *t > 1.0);
        self.duration = 0.0;
        self.position = 0.0;
        self.drag = None;
        self.menu = Menu::Closed;
        self.up_next = None;
        self.up_next_cancelled = false;
        self.bezel = None;
        self.controls = true;
        pixels().set(1, 1, vec![0, 0, 0, 255]);
        self.progress.set([0.0, 0.0, 0.0, 3.0]);
        self.buffered.set([0.0, 0.0, 0.0, 3.0]);
        self.clock.set("0:00 / 0:00");
        self.shown = Shown {
            status: Status::Loading,
            ..self.shown.clone()
        };
        self.shown.has_picture = false;
        self.error = None;
        want(id);
        WANT_FIRST.store(true, Ordering::Release);
        FIRST_PICTURE.store(false, Ordering::Release);
        send(Cmd::Load {
            id: id.to_string(),
            target: self.box_px,
        });
    }

    /// Retry after a playback failure: from where it was.
    pub fn retry(&mut self) {
        if let Some(id) = self.id.clone() {
            let at = Some(self.position);
            self.play(&id, at);
        }
    }

    /// Stops playback; returns where it was.
    pub fn stop(&mut self) -> Option<f64> {
        if !self.active {
            return None;
        }
        self.active = false;
        let pos = (self.duration > 0.0 || self.position > 0.0).then_some(self.position);
        want("");
        send(Cmd::Stop);
        WANT_FIRST.store(false, Ordering::Release);
        self.shown.status = Status::Idle;
        self.shown.has_picture = false;
        self.up_next = None;
        self.menu = Menu::Closed;
        self.drag = None;
        pos.or(self.resume_at)
    }

    fn with(&self, f: impl FnOnce(&Player) + Send + 'static) {
        send(Cmd::Do(Box::new(f)));
    }

    pub fn toggle_pause(&mut self) {
        if self.shown.status == Status::Ended {
            self.up_next = None;
            self.up_next_cancelled = false;
        }
        self.with(move |p| p.toggle_pause());
    }

    pub fn seek(&mut self, t: f64) {
        self.with(move |p| p.seek(t.max(0.0)));
    }

    pub fn seek_by(&mut self, dt: f64) {
        self.with(move |p| p.seek_by(dt));
    }

    pub fn seek_frac(&mut self, f: f32) {
        if self.duration > 0.0 && !self.shown.is_live {
            let t = f.clamp(0.0, 1.0) as f64 * self.duration;
            self.with(move |p| p.seek(t));
        }
    }

    pub fn set_volume(&mut self, v: f64) {
        let muted = self.shown.muted;
        self.with(move |p| {
            p.set_volume(v);
            if v > 0.0 && muted {
                // Core keeps test runs muted whatever this asks.
                p.set_muted(false);
            }
        });
    }

    pub fn volume(&self) -> f64 {
        self.shown.volume as f64
    }

    pub fn toggle_mute(&mut self) {
        let m = !self.shown.muted;
        self.with(move |p| p.set_muted(m));
    }

    pub fn set_speed(&mut self, s: f64) {
        self.with(move |p| p.set_speed(s));
    }

    /// One step along the speed menu's list.
    pub fn step_speed(&mut self, up: bool) {
        let cur = self.shown.speed();
        let i = SPEEDS
            .iter()
            .position(|s| (s - cur).abs() < 0.01)
            .unwrap_or(3);
        let j = if up {
            (i + 1).min(SPEEDS.len() - 1)
        } else {
            i.saturating_sub(1)
        };
        if let Some(s) = SPEEDS.get(j) {
            self.set_speed(*s);
        }
    }

    pub fn set_quality(&mut self, q: Option<u32>) {
        self.with(move |p| p.set_quality(q));
        self.shown.quality = q;
    }

    /// The drawn picture box (physical pixels). Core picks Auto quality from it and renders at
    /// most this size; it is sent on every change, including fullscreen and theatre.
    pub fn set_box(&mut self, w: f32, h: f32, scale: f32) {
        self.seek_w = (w - 24.0).max(10.0);
        let px = ((w * scale).round() as u32, (h * scale).round() as u32);
        if px != self.box_px && px.0 > 0 && px.1 > 0 {
            self.box_px = px;
            self.with(move |p| p.set_target_size(px.0, px.1));
        }
    }

    /// The player's state changed: move the live values. Returns (view must re-run, the video
    /// just ended).
    pub fn sync(&mut self) -> (bool, bool) {
        let Some(p) = current() else {
            return (false, false);
        };
        if !self.active {
            return (false, false);
        }
        let st: PlaybackState = p.state();
        // The command thread may not have loaded the new video yet: until the player's source is
        // ours, its state is the previous video's.
        if st.source.as_deref() != self.id.as_deref() {
            return (false, false);
        }
        self.duration = st.duration;
        if matches!(st.status, Status::Ready | Status::Ended) {
            self.position = st.position;
        }
        if st.status == Status::Ready
            && let Some(t) = self.resume_at.take()
        {
            self.with(move |p| p.seek(t));
        }
        if self.drag.is_none() {
            self.place(st.position, st.buffered_ahead);
        }
        let clock = if st.is_live {
            format::clock(st.position)
        } else {
            format!(
                "{} / {}",
                format::clock(st.position),
                format::clock(st.duration)
            )
        };
        self.clock.set(clock);
        let ended_before = self.shown.status == Status::Ended;
        let shown = Shown {
            status: st.status.clone(),
            paused: st.paused,
            buffering: st.buffering,
            muted: st.muted,
            volume: st.volume.round() as i32,
            has_picture: p.frame().is_some(),
            speed: (st.speed * 100.0).round() as i32,
            is_live: st.is_live,
            height: st.height,
            heights: st.heights.clone(),
            quality: p.quality(),
        };
        let ended_now = shown.status == Status::Ended && !ended_before;
        if shown != self.shown {
            self.shown = shown;
            return (true, ended_now);
        }
        (false, ended_now)
    }

    /// The scrubber's fill and buffered bar for a position (whole points: an unchanged width
    /// is an equal value, which wakes no frame).
    fn place(&mut self, pos: f64, ahead: f64) {
        let frac = |t: f64| {
            if self.duration > 0.0 {
                (t / self.duration).clamp(0.0, 1.0) as f32
            } else {
                0.0
            }
        };
        let w = self.seek_w;
        let played = (w * frac(pos)).round();
        self.progress.set([0.0, 0.0, played, 5.0]);
        self.buffered
            .set([0.0, 0.0, (w * frac(pos + ahead)).round(), 5.0]);
        self.knob.set([played - 6.5, -4.0, 13.0, 13.0]);
    }

    /// The pointer over the scrubber at `x` points: the tooltip, and the drag target if dragging.
    pub fn scrub_at(&mut self, x: f32) {
        let f = (x / self.seek_w.max(1.0)).clamp(0.0, 1.0);
        let t = f as f64 * self.duration;
        self.tip_text.set(format::clock(t));
        self.tip
            .set([(x - 24.0).clamp(-12.0, self.seek_w - 36.0), 0.0]);
        if self.drag.is_some() {
            self.drag = Some(f);
            self.place(t, 0.0);
        }
    }

    pub fn played_fraction(&self) -> Option<f32> {
        (self.duration > 0.0 && !self.shown.is_live)
            .then(|| (self.position / self.duration).clamp(0.0, 1.0) as f32)
    }

    /// Whether the controls show: always while not playing, while a menu is open or the
    /// scrubber is held, else for 3 s after the pointer last moved over the player.
    pub fn want_controls(&self, now: f64) -> bool {
        !self.shown.playing()
            || self.menu != Menu::Closed
            || self.drag.is_some()
            || (self.pointer_in && now - self.last_move < CONTROLS_HIDE)
    }

    /// The click bezel's fade: 500 ms.
    pub fn flash_bezel(&mut self, play_icon: bool) {
        self.bezel = Some((
            play_icon,
            Tween::linear(1.0, 0.0, Duration::from_millis(500)),
        ));
    }
}
