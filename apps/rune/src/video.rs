//! Playback through `nt_core::Player` (libmpv's software renderer, RGBA).
//!
//! How a frame reaches the screen: mpv renders into one of core's pooled buffers on the
//! player's render thread, then calls our waker *on that thread*. The waker compares
//! `frame_generation()` with the last one it uploaded and, only when it changed, copies the
//! frame into a `rune::Pixels` (`Pixels::set`, one memcpy, still off the UI thread). `set` wakes
//! only the window showing the image; Rune's next frame copies it into its image page (a
//! same-size frame reuses its slot) and uploads it to the GPU. No view commit happens for a
//! video frame. Position, buffered and the clock are `Live` values the waker's coalesced
//! `update_if` sets; only discrete changes (play/pause, loading, mute, volume, errors) re-run
//! the view.

use crate::app::{handle, mark_next};
use nt_core::{PixelFormat, PlaybackState, Player, Status, format, library};
use rune::{Key, Live, LiveText, Pixels, Tx};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

static PLAYER: OnceLock<Result<Arc<Player>, String>> = OnceLock::new();
static PIXELS: OnceLock<Pixels> = OnceLock::new();
static LAST_GEN: AtomicU64 = AtomicU64::new(0);
/// Set by `load`; the first picture after it emits `playback_started`.
static WANT_FIRST: AtomicBool = AtomicBool::new(false);
/// An `update_if` is already queued for the UI thread.
static SYNC_QUEUED: AtomicBool = AtomicBool::new(false);

/// Test and benchmark runs (`NT_MUTE=1`, set by tools/headless.sh) must never make sound:
/// there, mute can be turned on but never off.
fn may_unmute() -> bool {
    !std::env::var("NT_MUTE").is_ok_and(|v| v != "0")
}


pub fn pixels() -> &'static Pixels {
    PIXELS.get_or_init(Pixels::new)
}

fn waker() {
    let Some(Ok(p)) = PLAYER.get() else { return };
    let g = p.frame_generation();
    if g != LAST_GEN.load(Ordering::Acquire) {
        LAST_GEN.store(g, Ordering::Release);
        if let Some(f) = p.frame() {
            pixels().set(f.width, f.height, f.pixels().to_vec());
            if WANT_FIRST.swap(false, Ordering::AcqRel) {
                mark_next(nt_core::metrics::marks::PLAYBACK_STARTED);
            }
        }
    }
    if !SYNC_QUEUED.swap(true, Ordering::AcqRel)
        && let Some(h) = handle()
    {
        h.update_if(|s, tx| {
            SYNC_QUEUED.store(false, Ordering::Release);
            s.video.sync(tx)
        });
    }
}

fn player() -> Result<&'static Arc<Player>, String> {
    PLAYER
        .get_or_init(|| Player::new(PixelFormat::Rgba, waker).map(Arc::new).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

/// What the view shows about playback. Only discrete parts trigger a commit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub status: Status,
    pub paused: bool,
    pub buffering: bool,
    pub muted: bool,
    pub volume: i32,
    pub has_picture: bool,
}

pub struct VideoState {
    pub shown: Shown,
    pub error: Option<String>,
    /// Seek bar fill and buffered-ahead, `[x, y, w, h]` in the track.
    pub progress: Live<[f32; 4]>,
    pub buffered: Live<[f32; 4]>,
    pub clock: LiveText,
    pub seek_w: f32,
    duration: f64,
    position: f64,
    resume_at: Option<f64>,
    box_px: (u32, u32),
    active: bool,
    /// Whether mpv is muted. Kept here because `PlaybackState::muted` resets to false on
    /// `Player::stop` while mpv stays muted (see NOTES.md, core requests). Every mute change
    /// goes through `set_muted`, and mpv starts muted exactly when `NT_MUTE` is set.
    muted: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        VideoState {
            shown: Shown::default(),
            error: None,
            progress: Live::new([0.0, 0.0, 0.0, 4.0]),
            buffered: Live::new([0.0, 0.0, 0.0, 4.0]),
            clock: LiveText::new("0:00 / 0:00"),
            seek_w: 100.0,
            duration: 0.0,
            position: 0.0,
            resume_at: None,
            box_px: (0, 0),
            active: false,
            muted: !may_unmute(),
        }
    }
}

pub const CLOCK_PLACEHOLDER: &str = "00:00:00 / 00:00:00";

impl VideoState {
    fn set_muted(&mut self, p: &Player, muted: bool) {
        if muted || may_unmute() {
            p.set_muted(muted);
            self.muted = muted;
        }
    }

    /// Loads and plays `id`, from `resume_at` when returning to it.
    pub fn play(&mut self, id: &str, resume_at: Option<f64>) {
        match player() {
            Ok(p) => {
                self.error = None;
                WANT_FIRST.store(true, Ordering::Release);
                pixels().set(1, 1, vec![0, 0, 0, 255]);
                if let Err(e) = p.load(id) {
                    self.error = Some(e.to_string());
                }
                p.set_volume(library().volume());
                self.resume_at = resume_at.filter(|t| *t > 1.0);
                self.active = true;
                self.shown = Shown { status: Status::Loading, ..Shown::default() };
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Stops playback; returns where it was.
    pub fn stop(&mut self) -> Option<f64> {
        if !self.active {
            return None;
        }
        self.active = false;
        let pos = (self.duration > 0.0 || self.position > 0.0).then_some(self.position);
        if let Ok(p) = player() {
            p.stop();
        }
        WANT_FIRST.store(false, Ordering::Release);
        self.shown = Shown::default();
        pos.or(self.resume_at)
    }

    pub fn toggle_pause(&mut self) {
        if let Ok(p) = player() {
            p.toggle_pause();
        }
    }

    pub fn seek_frac(&mut self, f: f32) {
        if let Ok(p) = player()
            && self.duration > 0.0
        {
            p.seek(f.clamp(0.0, 1.0) as f64 * self.duration);
        }
    }

    pub fn set_volume(&mut self, v: f64) {
        if let Ok(p) = player() {
            p.set_volume(v);
            if v > 0.0 && self.muted {
                self.set_muted(p, false);
            }
        }
    }

    pub fn toggle_mute(&mut self) {
        if let Ok(p) = player() {
            self.set_muted(p, !self.muted);
        }
    }

    /// Watch-page keys (no text field focused).
    pub fn key(&mut self, key: &Key) -> bool {
        let Ok(p) = player() else { return false };
        let vol = p.state().volume;
        match key {
            Key::Text(t) if t == " " || t == "k" => p.toggle_pause(),
            Key::Text(t) if t == "j" => p.seek_by(-10.0),
            Key::Text(t) if t == "l" => p.seek_by(10.0),
            Key::Text(t) if t == "m" => self.set_muted(p, !self.muted),
            Key::Left => p.seek_by(-5.0),
            Key::Right => p.seek_by(5.0),
            Key::Up => self.set_volume((vol + 5.0).min(100.0)),
            Key::Down => self.set_volume((vol - 5.0).max(0.0)),
            _ => return false,
        }
        true
    }

    /// The player box on the watch page (layout points) and the window's scale.
    pub fn set_box(&mut self, r: [f32; 4], scale: f32) {
        self.seek_w = (r[2] - 24.0).max(10.0);
        let px = ((r[2] * scale) as u32, (r[3] * scale) as u32);
        if px != self.box_px && px.0 > 0 && px.1 > 0 {
            self.box_px = px;
            if let Ok(p) = player() {
                p.set_target_size(px.0, px.1);
            }
        }
    }

    /// The player's state changed: move the live values; true when the view must re-run.
    pub fn sync(&mut self, _tx: &mut Tx) -> bool {
        let Ok(p) = player() else { return false };
        if !self.active {
            return false;
        }
        let st: PlaybackState = p.state();
        self.duration = st.duration;
        if st.status == Status::Ready || st.status == Status::Ended {
            self.position = st.position;
        }
        if st.status == Status::Ready
            && let Some(t) = self.resume_at.take()
        {
            p.seek(t);
        }
        let frac = |t: f64| if st.duration > 0.0 { (t / st.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        // Whole points: an unchanged width is an equal value, which wakes no frame (mpv
        // reports the cache growing many times a second, also while paused).
        self.progress.set([0.0, 0.0, (self.seek_w * frac(st.position)).round(), 4.0]);
        self.buffered.set([0.0, 0.0, (self.seek_w * frac(st.position + st.buffered_ahead)).round(), 4.0]);
        self.clock.set(format!("{} / {}", format::clock(st.position), format::clock(st.duration)));
        let shown = Shown {
            status: st.status.clone(),
            paused: st.paused,
            buffering: st.buffering,
            muted: self.muted,
            volume: st.volume.round() as i32,
            has_picture: p.frame().is_some(),
        };
        if shown != self.shown {
            self.shown = shown;
            return true;
        }
        false
    }

    pub fn volume(&self) -> f64 {
        self.shown.volume as f64
    }
}
