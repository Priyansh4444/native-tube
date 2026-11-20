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
use nt_core::{
    AudioTrack, CaptionLanguage, CaptionTrack, HeatPoint, PixelFormat, PlaybackState, Player,
    Status, Storyboard, StoryboardFrame, format,
};
use rune::{Live, LiveText, Pixels, Tween};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The player, made on the first video. A failed start leaves it empty, so the next video
/// (or Retry) tries again rather than disabling playback for the session.
static PLAYER: OnceLock<Arc<Player>> = OnceLock::new();
static SYNC_QUEUED: AtomicBool = AtomicBool::new(false);

/// Frame publication and the next scene switch share this lock. Copies stay outside it.
#[derive(Default)]
struct Surface {
    serial: u64,
    loaded: u64,
    generation: u64,
    pixels: Option<Pixels>,
}

static SURFACE: Mutex<Surface> = Mutex::new(Surface {
    serial: 0,
    loaded: 0,
    generation: 0,
    pixels: None,
});

type Job = Box<dyn FnOnce(&Player) + Send>;

enum Cmd {
    Load {
        id: String,
        serial: u64,
        paused: bool,
        start: f64,
        target: (u32, u32),
    },
    Stop,
    Shutdown(mpsc::Sender<()>),
    Do {
        serial: u64,
        job: Job,
    },
    /// Start libmpv (and the GPU surface) ahead of the first video.
    Warm,
}

impl Cmd {
    fn is_load_or_stop(&self) -> bool {
        matches!(self, Cmd::Load { .. } | Cmd::Stop)
    }
}

static QUEUE: OnceLock<mpsc::Sender<Cmd>> = OnceLock::new();

fn send(cmd: Cmd) {
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let started = std::thread::Builder::new()
            .name("nt-player-cmd".into())
            .spawn(move || command_loop(rx));
        if let Err(e) = started {
            eprintln!("nt-rune: can't start the player thread: {e}");
        }
        tx
    });
    let _ = tx.send(cmd);
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
        Cmd::Load {
            id,
            serial,
            paused,
            start,
            target,
        } => {
            let result = player().and_then(|p| {
                if !ATTACH_ASKED.swap(true, Ordering::AcqRel) {
                    let t = std::time::Instant::now();
                    attach_before_first_load();
                    if std::env::var_os("NT_GPU_TRACE").is_some() {
                        eprintln!(
                            "GPUTRACE attach t={:.3} took={:.1}ms",
                            crate::scroller::now(),
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
                let t = std::time::Instant::now();
                p.load_at(&id, start).map_err(|e| e.to_string())?;
                if std::env::var_os("NT_GPU_TRACE").is_some() {
                    eprintln!(
                        "GPUTRACE load_at t={:.3} took={:.1}ms",
                        crate::scroller::now(),
                        t.elapsed().as_secs_f64() * 1000.0
                    );
                }
                if target.0 > 0 && target.1 > 0 {
                    p.set_target_size(target.0, target.1);
                }
                if paused {
                    p.pause();
                }
                Ok(())
            });
            match result {
                Ok(()) => {
                    surface().loaded = serial;
                    waker();
                }
                Err(msg) => {
                    eprintln!("nt-rune: playback of {id} failed: {msg}");
                    if let Some(h) = handle() {
                        h.update_if(move |s, _| {
                            if s.video.serial != serial {
                                return false;
                            }
                            s.video.error = Some(msg);
                            true
                        });
                    }
                }
            }
        }
        Cmd::Warm => {
            if PLAYER.get().is_none()
                && let Err(e) = player()
            {
                eprintln!("nt-rune: player warm-up: {e}");
            }
        }
        Cmd::Stop => {
            if let Some(p) = PLAYER.get() {
                p.stop();
            }
        }
        Cmd::Shutdown(done) => {
            shutdown_player();
            let _ = done.send(());
        }
        Cmd::Do { serial, job } => {
            let applies = {
                let surface = surface();
                serial == surface.serial && serial == surface.loaded
            };
            if let Some(p) = PLAYER.get().filter(|_| applies) {
                job(p);
            }
        }
    }
}

fn surface() -> std::sync::MutexGuard<'static, Surface> {
    SURFACE.lock().unwrap_or_else(|e| e.into_inner())
}

fn waker() {
    let Some(p) = PLAYER.get() else { return };
    let generation = p.frame_generation();
    let serial = {
        let surface = surface();
        (surface.loaded == surface.serial && generation > surface.generation)
            .then_some(surface.serial)
    };
    if let Some(serial) = serial
        && let Some(frame) = p.frame()
    {
        let pixels = frame.pixels().to_vec();
        let mut surface = surface();
        if surface.serial == serial
            && surface.loaded == serial
            && frame.generation > surface.generation
            && let Some(out) = &surface.pixels
        {
            out.set(frame.width, frame.height, pixels);
            surface.generation = frame.generation;
        }
    }
    if let Some(h) = handle()
        && !SYNC_QUEUED.swap(true, Ordering::AcqRel)
    {
        h.update_if(|s, tx| {
            SYNC_QUEUED.store(false, Ordering::Release);
            s.player_changed(tx)
        });
    }
}

/// Only the command thread initializes mpv. The UI and waker never wait on its startup.
fn player() -> Result<Arc<Player>, String> {
    if let Some(p) = PLAYER.get() {
        return Ok(p.clone());
    }
    if std::env::var_os("NT_GPU_TRACE").is_some() {
        eprintln!("GPUTRACE player_new t={:.3}", crate::scroller::now());
    }
    let p = Arc::new(Player::new(PixelFormat::Rgba, waker).map_err(|e| e.to_string())?);
    let _ = PLAYER.set(p.clone());
    Ok(p)
}

#[cfg(target_os = "linux")]
pub fn player_if_started() -> Option<&'static Player> {
    PLAYER.get().map(Arc::as_ref)
}

/// Whether this load's picture is really on the GPU surface: core counts surface frames
/// that carried a decoded picture since mpv's `playback-restart` for the current file
/// (`Player::presented_frames`, reset by every load). Core reports Ready and maps the surface
/// with one render before anything is decoded; that render is black and doesn't count.
pub fn gpu_picture(serial: u64) -> bool {
    let Some(p) = PLAYER.get() else { return false };
    let applied = {
        let surface = surface();
        surface.loaded == serial && surface.serial == serial
    };
    applied && p.presented_frames() > 0
}

/// The first load asked the UI thread to attach the GPU surface (it does so once).
static ATTACH_ASKED: AtomicBool = AtomicBool::new(false);

/// Starts libmpv on the command thread once the home page has drawn, so the first click
/// doesn't pay for it (~105 ms for `Player::new`, measured). The GPU surface still attaches on
/// the first load: attaching it here too saved another ~60 ms but cost 75 MiB of idle PSS
/// (EGL and the GL driver), over the idle budget. `NT_NO_PREWARM` turns it off. It resolves
/// nothing and loads nothing.
pub fn prewarm() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("NT_NO_PREWARM").is_none() {
        ONCE.call_once(|| send(Cmd::Warm));
    }
}

/// On the command thread, once, between creating the player and its first load: has the UI
/// thread (which owns the window) attach the GPU surface, and waits for it (at most 1 s).
fn attach_before_first_load() {
    #[cfg(target_os = "linux")]
    if crate::wayland_video::enabled()
        && let Some(h) = handle()
    {
        let (done, wait) = mpsc::channel::<()>();
        h.update_if(move |s, _| {
            let scale = s.scale.get();
            s.native_video.try_attach(scale);
            let _ = done.send(());
            false
        });
        let _ = wait.recv_timeout(Duration::from_secs(1));
    }
}

fn shutdown_player() {
    if let Some(player) = PLAYER.get() {
        player.shutdown();
    } else {
        nt_core::library().flush();
    }
}

/// Wait for the final checkpoint after any queued playback commands, even if the UI already
/// left Watch. The static player is never dropped.
pub fn checkpoint_on_exit(s: &mut crate::app::App, _: &mut rune::Tx) {
    let (done, wait) = mpsc::channel();
    let queued = QUEUE
        .get()
        .is_some_and(|queue| queue.send(Cmd::Shutdown(done)).is_ok());
    if !queued || wait.recv().is_err() {
        shutdown_player();
    }
    #[cfg(target_os = "linux")]
    s.native_video.detach();
    s.video.active = false;
}

/// "Stats for nerds": what the player reports now, one line per number.
fn stats(p: &Player, st: &PlaybackState, box_px: (u32, u32)) -> [String; 9] {
    let r = p.render_stats();
    let avg = if r.frames > 0 {
        r.total_render_ms / r.frames as f64
    } else {
        0.0
    };
    let size = st
        .video_size
        .map_or("-".to_string(), |(w, h)| format!("{w}x{h}"));
    let quality = p.quality().map_or("Auto".to_string(), |h| format!("{h}p"));
    let speed = if st.speed.is_finite() && st.speed > 0.0 {
        st.speed
    } else {
        1.0
    };
    let height = if st.height == 0 {
        st.video_size.map_or(0, |(_, h)| h)
    } else {
        st.height
    };
    [
        format!("Video ID  {}", st.source.as_deref().unwrap_or("-")),
        format!("Viewport  {}x{} px", box_px.0, box_px.1),
        format!("Current / Optimal Res  {size} / {height}p ({quality})"),
        format!("Frames  {} rendered", r.frames),
        format!("Render  {avg:.2} ms avg, {:.2} ms max", r.max_render_ms),
        format!("Speed  {speed:.2}x"),
        format!(
            "Volume  {}%{}",
            st.volume.round(),
            if st.muted { " (muted)" } else { "" }
        ),
        format!("Buffer Health  {:.2} s", st.buffered_ahead),
        format!(
            "Position  {} / {}",
            format::clock(st.position),
            format::clock(st.duration)
        ),
    ]
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
    /// Caption tracks offered, and the one shown (track index, translation language).
    pub cc_count: usize,
    pub cc_sel: Option<(usize, Option<String>)>,
    pub audio: Option<String>,
    pub normalize: bool,
    pub normalization_supported: bool,
    pub looping: bool,
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
    Captions,
    /// Auto-translate for this caption track.
    Translate(usize),
    Sleep,
    Audio,
}

/// The sleep timer: off, a deadline (seconds on `now()`, and the minutes it was set for), or
/// at the end of this video.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Sleep {
    #[default]
    Off,
    At(f64, u32),
    EndOfVideo,
}

pub const SLEEP_MINUTES: [u32; 6] = [10, 15, 20, 30, 45, 60];

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
    pub serial: u64,
    first_generation: u64,
    pub picture: Pixels,
    pub poster_frame: Option<Pixels>,
    pub audio_tracks: Vec<AudioTrack>,
    storyboard: Option<Storyboard>,
    preview_key: Option<StoryboardFrame>,
    preview_generation: u64,
    pub preview: Pixels,
    pub tip_chapter: LiveText,
    pub captions: Vec<CaptionTrack>,
    pub caption_languages: Vec<CaptionLanguage>,
    /// The watch info's chapters (start seconds, title) and replay heatmap, for this video.
    pub chapters: Vec<(f64, String)>,
    pub heat: Vec<HeatPoint>,
    /// The chapter under the playhead, as the bar shows it ("Intro ›").
    pub chapter_text: LiveText,
    pub chapter_idx: Option<usize>,
    pub sleep: Sleep,
    /// "Stats for nerds": open, and its live text.
    pub stats_open: bool,
    pub stats_rows: [LiveText; 9],
    prepared: Option<(String, Option<u32>, (u32, u32))>,
    /// This load has actually been drawn in the visible player.
    pub first_shown: Cell<bool>,
    /// The picture's width / height once known (captions follow the picture, not the box).
    pub aspect: Option<f32>,
    /// Caption lift: 0 at rest (about 8% above the picture's bottom), 1 clear of the bars;
    /// the running ease (from, to, start on `now()`), and the last `sub-pos` sent.
    pub caption_lift: f32,
    pub caption_anim: Option<(f32, f32, f64)>,
    pub caption_pos: f64,
    /// Test-only (`NT_TEST_CYCLE=controls`): the controls held shown.
    pub test_controls: bool,
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
            serial: 0,
            first_generation: 0,
            picture: Pixels::new(),
            poster_frame: None,
            audio_tracks: Vec::new(),
            storyboard: None,
            preview_key: None,
            preview_generation: 0,
            preview: Pixels::new(),
            tip_chapter: LiveText::new(""),
            captions: Vec::new(),
            caption_languages: Vec::new(),
            chapters: Vec::new(),
            heat: Vec::new(),
            chapter_text: LiveText::new(""),
            chapter_idx: None,
            sleep: Sleep::Off,
            stats_open: false,
            stats_rows: std::array::from_fn(|_| LiveText::new("")),
            prepared: None,
            first_shown: Cell::new(false),
            aspect: None,
            caption_lift: 0.0,
            caption_anim: None,
            caption_pos: f64::NAN,
            test_controls: false,
        }
    }
}

pub const CLOCK_PLACEHOLDER: &str = "00:00:00 / 00:00:00";
pub const CONTROLS_HIDE: f64 = 3.0;

impl VideoState {
    /// Loads and plays `id`, from `resume_at` when returning to it. Never shows the previous
    /// video's picture: the image is cleared first, and core publishes frames of the new video
    /// only.
    pub fn play(&mut self, id: &str, resume_at: Option<f64>, paused: bool, duration: f64) {
        self.id = Some(id.to_string());
        self.active = true;
        self.resume_at = resume_at;
        self.duration = duration;
        self.position = resume_at.unwrap_or(0.0);
        self.drag = None;
        self.menu = Menu::Closed;
        self.up_next = None;
        self.up_next_cancelled = false;
        self.bezel = None;
        self.controls = true;
        self.picture = Pixels::new();
        self.poster_frame = None;
        {
            let mut surface = surface();
            surface.serial += 1;
            self.serial = surface.serial;
            self.first_generation = surface.generation;
            surface.pixels = Some(self.picture.clone());
        }
        self.scrub_hover = false;
        self.vol_hover = false;
        self.clear_preview();
        self.storyboard = None;
        self.prepared = None;
        self.progress.set([0.0, 0.0, 0.0, 3.0]);
        self.buffered.set([0.0, 0.0, 0.0, 3.0]);
        self.clock.set(format!(
            "{} / {}",
            format::clock(self.position),
            format::clock(duration)
        ));
        self.shown = Shown {
            status: Status::Loading,
            paused,
            muted: self.shown.muted,
            volume: self.shown.volume,
            normalize: self.shown.normalize,
            normalization_supported: self.shown.normalization_supported,
            ..Shown::default()
        };
        self.shown.has_picture = false;
        self.first_shown.set(false);
        self.aspect = None;
        self.caption_pos = f64::NAN;
        self.error = None;
        self.audio_tracks.clear();
        self.captions.clear();
        self.caption_languages.clear();
        self.chapters.clear();
        self.heat.clear();
        self.chapter_text.set("");
        self.chapter_idx = None;
        if self.sleep == Sleep::EndOfVideo {
            self.sleep = Sleep::Off;
        }
        // Back to a watch page starts where it stopped (core's `load_at`), not at 0:00 then a seek.
        send(Cmd::Load {
            id: id.to_string(),
            serial: self.serial,
            paused,
            start: self.resume_at.unwrap_or(0.0),
            target: self.box_px,
        });
    }

    /// Retry after a playback failure: from where it was.
    pub fn retry(&mut self) {
        if let Some(id) = self.id.clone() {
            let at = Some(self.position);
            self.play(&id, at, self.shown.paused, self.duration);
        }
    }

    /// Retires the picture and saves the position. A following load replaces the stream.
    pub fn leave(&mut self, continuing_watch: bool) -> Option<f64> {
        if !self.active {
            return None;
        }
        self.active = false;
        let pos = (self.duration > 0.0 || self.position > 0.0).then_some(self.position);
        {
            let mut surface = surface();
            surface.serial += 1;
            surface.pixels = None;
        }
        // An intervening mpv stop would cancel preparation of the successor.
        if !continuing_watch {
            send(Cmd::Stop);
        }
        self.shown.status = Status::Idle;
        self.shown.has_picture = false;
        self.up_next = None;
        self.menu = Menu::Closed;
        self.drag = None;
        self.resume_at.or(pos)
    }

    fn with(&self, f: impl FnOnce(&Player) + Send + 'static) {
        send(Cmd::Do {
            serial: self.serial,
            job: Box::new(f),
        });
    }

    pub fn toggle_pause(&mut self) {
        if self.shown.status == Status::Ended {
            self.up_next = None;
            self.up_next_cancelled = false;
        }
        self.with(move |p| p.toggle_pause());
    }

    pub fn pause(&self) {
        self.with(Player::pause);
    }

    pub fn toggle_loop(&self) {
        let on = !self.shown.looping;
        self.with(move |p| p.set_loop(on));
    }

    pub fn toggle_normalize(&self) {
        let on = !self.shown.normalize;
        self.with(move |p| p.set_normalize(on));
    }

    pub fn audio_track(&self, id: String) {
        let serial = self.serial;
        self.with(move |p| {
            p.set_audio_track(&id)
                .on_done(move |r| playback_option(serial, "Audio track", r));
        });
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
        self.prepared = None;
        self.with(move |p| p.set_quality(q));
        self.shown.quality = q;
    }

    pub fn prepare_next(&mut self, id: Option<&str>) {
        let candidate = id.map(|id| (id.to_string(), self.shown.quality, self.box_px));
        if self.prepared != candidate {
            self.prepared = candidate;
            let id = id.map(str::to_string);
            self.with(move |p| p.prepare_next(id.as_deref()));
        }
    }

    /// The drawn picture box (physical pixels). Core picks Auto quality from it and renders at
    /// most this size; it is sent on every change, including fullscreen and theatre.
    pub fn set_box(&mut self, w: f32, h: f32, scale: f32) -> bool {
        let seek_w = (w - 24.0).max(10.0);
        let changed = self.seek_w != seek_w;
        self.seek_w = seek_w;
        let px = ((w * scale).round() as u32, (h * scale).round() as u32);
        if px != self.box_px && px.0 > 0 && px.1 > 0 {
            self.box_px = px;
            self.with(move |p| p.set_target_size(px.0, px.1));
        }
        changed
    }

    /// The player's state changed: move the live values. Returns (view must re-run, the video
    /// just ended).
    pub fn sync(&mut self) -> (bool, bool) {
        let Some(p) = PLAYER.get() else {
            return (false, false);
        };
        if !self.active {
            return (false, false);
        }
        let (loaded, has_picture) = {
            let surface = surface();
            (surface.loaded, surface.generation > self.first_generation)
        };
        if loaded != self.serial {
            return (false, false);
        }
        let st: PlaybackState = p.state();
        // The command thread may not have loaded the new video yet: until the player's source is
        // ours, its state is the previous video's.
        if st.source.as_deref() != self.id.as_deref() {
            return (false, false);
        }
        if st.duration > 0.0 {
            self.duration = st.duration;
        }
        self.aspect = st.aspect();
        if matches!(st.status, Status::Ready | Status::Ended) {
            self.position = st.position;
        }
        if st.status == Status::Ready {
            self.resume_at = None;
        }
        if self.drag.is_none() {
            self.place(self.position, st.buffered_ahead);
        }
        let clock = if st.is_live {
            format::clock(self.position)
        } else {
            format!(
                "{} / {}",
                format::clock(self.position),
                format::clock(self.duration)
            )
        };
        self.clock.set(clock);
        let ended_before = self.shown.status == Status::Ended;
        if st.captions != self.captions {
            self.captions = st.captions.clone();
        }
        if st.caption_languages != self.caption_languages {
            self.caption_languages = st.caption_languages.clone();
        }
        let audio_changed = self.audio_tracks != st.audio_tracks;
        if audio_changed {
            self.audio_tracks.clone_from(&st.audio_tracks);
        }
        let storyboard_changed = self.storyboard != st.storyboard;
        if storyboard_changed {
            self.storyboard.clone_from(&st.storyboard);
            self.clear_preview();
        }
        self.update_chapter(self.position);
        if self.stats_open {
            for (line, label) in self.stats_rows.iter().zip(stats(p, &st, self.box_px)) {
                line.set(label);
            }
        }
        if has_picture {
            self.poster_frame = None;
        }
        let shown = Shown {
            status: st.status.clone(),
            paused: st.paused,
            buffering: st.buffering,
            muted: st.muted,
            volume: st.volume.round() as i32,
            has_picture,
            speed: (st.speed * 100.0).round() as i32,
            is_live: st.is_live,
            height: st.height,
            heights: st.heights.clone(),
            quality: p.quality(),
            cc_count: st.captions.len(),
            cc_sel: st.selected_captions.clone(),
            audio: st.audio_track.clone(),
            normalize: st.normalize,
            normalization_supported: st.normalization_supported,
            looping: st.looping,
        };
        let ended_now = shown.status == Status::Ended && !ended_before;
        if shown != self.shown {
            if let Status::Failed(error) = &shown.status
                && self.shown.status != shown.status
            {
                eprintln!("nt-rune: playback failed: {error}");
            }
            self.shown = shown;
            return (true, ended_now);
        }
        (audio_changed || storyboard_changed, ended_now)
    }

    /// The watch info for this video arrived: its chapters and heatmap.
    pub fn set_watch_info(
        &mut self,
        id: &str,
        chapters: &[nt_core::Chapter],
        heat: Vec<HeatPoint>,
    ) {
        if self.id.as_deref() != Some(id) {
            return;
        }
        let mut ch: Vec<(f64, String)> = chapters
            .iter()
            .map(|c| (c.start, c.title.clone()))
            .collect();
        ch.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.chapters = ch;
        self.heat = heat;
        self.chapter_idx = None;
        let pos = self.position;
        self.update_chapter(pos);
    }

    /// The chapter at `t`, by index.
    pub fn chapter_at(&self, t: f64) -> Option<usize> {
        self.chapters
            .partition_point(|(start, _)| *start <= t)
            .checked_sub(1)
    }

    fn update_chapter(&mut self, t: f64) {
        let idx = self.chapter_at(t);
        if idx != self.chapter_idx {
            self.chapter_idx = idx;
            let label = idx
                .and_then(|i| self.chapters.get(i))
                .map(|(_, title)| format!("•  {title}  ›"))
                .unwrap_or_default();
            self.chapter_text.set(label);
        }
    }

    /// Caption track `sel` (or off), then a view run when the player reports it.
    pub fn set_captions(&mut self, sel: Option<(usize, Option<String>)>) {
        let serial = self.serial;
        self.with(move |p| {
            p.set_captions(sel)
                .on_done(move |r| playback_option(serial, "Captions", r));
        });
    }

    /// mpv `sub-pos` for the current lift in a player box `box_w` × `box_h` (logical px).
    /// YouTube keeps captions about 8% above the picture's bottom and lifts them clear of the
    /// progress and control bars while those show. The bars sit on the box, the captions on
    /// the picture, so letterboxing (theatre, fullscreen) reduces how far they must rise.
    pub fn caption_percent(&self, box_w: f32, box_h: f32) -> f64 {
        // With sub-pos 100, core's `sub-margin-y` (44 of 720) leaves 6.1% under the block.
        const REST: f64 = 98.0;
        // Bar row 48 + scrubber 16 + 12 of air, from the box's bottom.
        const CONTROLS: f32 = 76.0;
        let aspect = self.aspect.unwrap_or(16.0 / 9.0).max(0.1);
        let vh = (box_w / aspect).min(box_h).max(1.0);
        let gap = ((box_h - vh) / 2.0).max(0.0);
        let needed = f64::from(((CONTROLS - gap) / vh).max(0.0)) * 100.0;
        let lifted = (100.0 - (needed - 100.0 * 44.0 / 720.0)).min(REST);
        REST + (lifted - REST) * f64::from(self.caption_lift)
    }

    /// Eases the lift toward the controls' state (200 ms, ease-out) and sends `sub-pos` when
    /// it moved. Returns whether the ease still runs (the caller keeps frames coming).
    pub fn place_captions(&mut self, box_w: f32, box_h: f32, now: f64) -> bool {
        let to = if self.controls && self.shown.status != Status::Idle {
            1.0
        } else {
            0.0
        };
        if self.caption_anim.map_or(self.caption_lift, |(_, t, _)| t) != to {
            self.caption_anim = Some((self.caption_lift, to, now));
        }
        if let Some((from, to, start)) = self.caption_anim {
            let t = (((now - start) / 0.2) as f32).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            self.caption_lift = from + (to - from) * eased;
            if t >= 1.0 {
                self.caption_anim = None;
            }
        }
        if self.shown.cc_sel.is_some() && box_w > 0.0 && box_h > 0.0 {
            let pos = self.caption_percent(box_w, box_h);
            if self.caption_pos.is_nan() || (pos - self.caption_pos).abs() >= 0.05 {
                self.caption_pos = pos;
                self.with(move |p| p.set_caption_position(pos));
            }
        }
        self.caption_anim.is_some()
    }

    /// The C key: captions on (the first track in the viewer's language, else the first) or off.
    pub fn toggle_captions(&mut self) {
        if self.shown.cc_sel.is_some() {
            self.set_captions(None);
        } else if !self.captions.is_empty() {
            let i = self
                .captions
                .iter()
                .position(|c| c.language_code.starts_with("en") && !c.auto_generated)
                .or_else(|| {
                    self.captions
                        .iter()
                        .position(|c| c.language_code.starts_with("en"))
                })
                .unwrap_or(0);
            self.set_captions(Some((i, None)));
        }
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
    pub fn scrub_at(&mut self, x: f32, scale: f32) {
        let f = (x / self.seek_w.max(1.0)).clamp(0.0, 1.0);
        let t = f as f64 * self.duration;
        self.tip_text.set(format::clock(t));
        let title = self
            .chapter_at(t)
            .map_or("", |i| self.chapters[i].1.as_str());
        self.tip_chapter.set(title);
        self.tip.set([
            (x - 80.0).clamp(-12.0, (self.seek_w - 148.0).max(-12.0)),
            0.0,
        ]);
        self.request_preview(t, scale);
        if self.drag.is_some() {
            self.drag = Some(f);
            self.place(t, 0.0);
        }
    }

    pub fn has_storyboard(&self) -> bool {
        self.storyboard.is_some()
    }

    pub fn clear_preview(&mut self) {
        self.preview_key = None;
        self.preview_generation += 1;
        self.preview = Pixels::new();
    }

    fn request_preview(&mut self, time: f64, scale: f32) {
        let Some(board) = &self.storyboard else {
            return;
        };
        let width = crate::view::px(160.0, scale);
        let key = board.level_for_width(width).frame_at(time);
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key);
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let job = board.preview(time, width);
        job.on_done(move |r| {
            if let (Ok(img), Some(h)) = (r, handle()) {
                let pixels = img.pixels.to_vec();
                h.update_if(move |s, _| {
                    let v = &s.video;
                    if v.preview_generation == generation && (v.scrub_hover || v.drag.is_some()) {
                        v.preview.set(img.width, img.height, pixels);
                    }
                    false
                });
            }
        });
    }

    pub fn played_fraction(&self) -> Option<f32> {
        (self.duration > 0.0 && !self.shown.is_live)
            .then(|| (self.position / self.duration).clamp(0.0, 1.0) as f32)
    }

    /// Whether the controls show: always while not playing, while a menu is open or the
    /// scrubber is held, else for 3 s after the pointer last moved over the player.
    pub fn want_controls(&self, now: f64) -> bool {
        self.test_controls
            || !self.shown.playing()
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

fn playback_option(serial: u64, name: &'static str, result: nt_core::Result<()>) {
    if let Err(error) = result
        && !matches!(error, nt_core::Error::Cancelled)
        && let Some(h) = handle()
    {
        h.update_if(move |s, tx| {
            if s.video.serial != serial {
                return false;
            }
            s.show_toast(&format!("{name}: {}", crate::app::explain(&error)), tx);
            true
        });
    }
}
