//! Things that happen off the UI thread and must reach `update`: decoded images, image
//! allocations and player wake-ups (one channel, read by a subscription), plus the autopilot's
//! command stream.
//!
//! The player lives in a process-wide slot (`player()`), so its waker can look at it on the
//! player's own threads and drop wake-ups that would change nothing on screen: a new video frame
//! while the player is scrolled out of the built band, or a state change in a field the UI does
//! not show. Every message makes iced rebuild the whole view, so this filter is what keeps a
//! playing video from costing a full view per frame when nobody sees it.

use crate::app::Message;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
use nt_core::{PixelFormat, PlaybackState, Player, RgbaImage};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub enum Ext {
    Image(String, u32, nt_core::Result<Arc<RgbaImage>>),
    Player,
    PlayerReady,
    Preview,
}

type Chan = (
    mpsc::UnboundedSender<Ext>,
    Mutex<Option<mpsc::UnboundedReceiver<Ext>>>,
);

fn chan() -> &'static Chan {
    static C: OnceLock<Chan> = OnceLock::new();
    C.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded();
        (tx, Mutex::new(Some(rx)))
    })
}

pub fn send(e: Ext) {
    let _ = chan().0.unbounded_send(e);
}

// ---- the player and its wake gate ---------------------------------------------------------------

static PLAYER: OnceLock<Player> = OnceLock::new();
/// Set while a player wake-up is queued, so a burst of frames becomes one message.
static PENDING: AtomicBool = AtomicBool::new(false);
/// Whether the picture is in the built band (or fullscreen); frames only wake the UI then.
static VISIBLE: AtomicBool = AtomicBool::new(true);
static LAST_GEN: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_KEY: AtomicU64 = AtomicU64::new(u64::MAX);

/// The one player, created on first use. A failed start is not remembered: the next call tries
/// again. It is never dropped (a waker on a player thread could otherwise drop the last
/// reference and make mpv join its own thread); quitting stops it and ends the process.
pub fn player() -> Result<&'static Player, String> {
    if let Some(p) = PLAYER.get() {
        return Ok(p);
    }
    // One creator at a time: the warm-up thread and a click can race.
    static CREATE: Mutex<()> = Mutex::new(());
    let _guard = CREATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = PLAYER.get() {
        return Ok(p);
    }
    let p = Player::new(PixelFormat::Rgba, wake).map_err(|e| e.to_string())?;
    Ok(PLAYER.get_or_init(|| p))
}

/// Start libmpv outside the UI thread after the first frame. Resolution is left to
/// the actual preload or click, so warming up never spends an extra YouTube request.
pub fn prewarm_player() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("NT_NO_PREWARM").is_none() {
        ONCE.call_once(|| {
            nt_core::runtime().spawn_blocking(|| match player() {
                Ok(_) => {
                    crate::scroller::trace_event("player_ready");
                    send(Ext::PlayerReady);
                }
                Err(e) => eprintln!("nt-iced: player warm-up: {e}"),
            });
        });
    }
}

pub fn refresh_player() {
    wake();
}

pub fn existing_player() -> Option<&'static Player> {
    PLAYER.get()
}

pub fn set_video_visible(visible: bool) {
    if VISIBLE.swap(visible, Ordering::AcqRel) != visible && visible {
        // Coming back into view: make sure the newest picture is taken.
        LAST_GEN.store(u64::MAX, Ordering::Release);
        wake();
    }
}

/// A digest of every state field the UI shows, at the precision it shows it.
fn state_key(s: &PlaybackState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(&s.status).hash(&mut h);
    if let nt_core::Status::Failed(m) = &s.status {
        m.hash(&mut h);
    }
    (s.position.max(0.0) as u64).hash(&mut h);
    (s.duration.max(0.0) as u64).hash(&mut h);
    (s.buffered_ahead.max(0.0) as u64).hash(&mut h);
    s.paused.hash(&mut h);
    s.buffering.hash(&mut h);
    (s.volume.round() as i64).hash(&mut h);
    s.muted.hash(&mut h);
    ((s.speed * 100.0).round() as i64).hash(&mut h);
    s.is_live.hash(&mut h);
    s.height.hash(&mut h);
    s.heights.hash(&mut h);
    s.video_size.hash(&mut h);
    s.source.hash(&mut h);
    s.captions.len().hash(&mut h);
    s.selected_captions.hash(&mut h);
    s.audio_tracks.len().hash(&mut h);
    s.audio_track.hash(&mut h);
    s.normalize.hash(&mut h);
    s.looping.hash(&mut h);
    s.storyboard.is_some().hash(&mut h);
    h.finish()
}

fn wake() {
    if let Some(p) = PLAYER.get() {
        let generation = p.frame_generation();
        // The first real frame on the GPU surface changes what the UI paints (the hole and
        // the poster), with no software frame and no state change to show for it. The count
        // is an atomic, so the waker never waits on the UI thread.
        let key = state_key(&p.state()) ^ u64::from(p.presented_frames() > 0);
        let new_frame =
            VISIBLE.load(Ordering::Acquire) && LAST_GEN.load(Ordering::Acquire) != generation;
        let changed = LAST_KEY.load(Ordering::Acquire) != key;
        if !new_frame && !changed {
            return;
        }
        if new_frame {
            LAST_GEN.store(generation, Ordering::Release);
        }
        LAST_KEY.store(key, Ordering::Release);
    }
    if !PENDING.swap(true, Ordering::AcqRel) {
        send(Ext::Player);
    }
}

/// Call before reading the player's state, so a wake that lands meanwhile queues again.
pub fn player_taken() {
    PENDING.store(false, Ordering::Release);
}

/// Force the next wake through (after a load, so the new video's first state is seen).
pub fn reset_gate() {
    LAST_GEN.store(u64::MAX, Ordering::Release);
    LAST_KEY.store(u64::MAX, Ordering::Release);
}

static PREVIEW_PLAYER: OnceLock<Player> = OnceLock::new();
static PREVIEW_ACTIVE: AtomicBool = AtomicBool::new(false);
static PREVIEW_PENDING: AtomicBool = AtomicBool::new(false);

pub fn init_preview() -> Result<(), String> {
    static CREATE: Mutex<()> = Mutex::new(());
    let _guard = CREATE.lock().unwrap_or_else(|e| e.into_inner());
    if PREVIEW_PLAYER.get().is_none() {
        let player = Player::new_preview(PixelFormat::Rgba, || {
            if PREVIEW_ACTIVE.load(Ordering::Acquire)
                && !PREVIEW_PENDING.swap(true, Ordering::AcqRel)
            {
                send(Ext::Preview);
            }
        })
        .map_err(|e| e.to_string())?;
        let _ = PREVIEW_PLAYER.set(player);
    }
    Ok(())
}

pub fn play_preview(id: &str, width: u32, height: u32) {
    if let Some(player) = PREVIEW_PLAYER.get() {
        player.set_target_size(width, height);
        PREVIEW_ACTIVE.store(true, Ordering::Release);
        if player.load(id).is_err() {
            PREVIEW_ACTIVE.store(false, Ordering::Release);
        }
    }
}

pub fn stop_preview() {
    if PREVIEW_ACTIVE.swap(false, Ordering::AcqRel)
        && let Some(player) = PREVIEW_PLAYER.get()
    {
        player.stop();
    }
    PREVIEW_PENDING.store(false, Ordering::Release);
}

pub fn preview_frame(id: &str) -> Option<Arc<nt_core::VideoFrame>> {
    PREVIEW_PENDING.store(false, Ordering::Release);
    let player = PREVIEW_PLAYER.get()?;
    if player.state().source.as_deref() == Some(id) {
        player.frame()
    } else {
        None
    }
}

// ---- streams ------------------------------------------------------------------------------------

pub fn events() -> impl Stream<Item = Message> {
    let rx = chan().1.lock().unwrap_or_else(|e| e.into_inner()).take();
    match rx {
        Some(rx) => rx.map(Message::Ext).boxed(),
        None => iced::futures::stream::pending().boxed(),
    }
}

pub fn autopilot() -> impl Stream<Item = Message> {
    iced::stream::channel(16, async |mut out: mpsc::Sender<Message>| {
        while let Some(cmd) = nt_core::autopilot::next_command().await {
            if out.send(Message::Pilot(cmd)).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
    })
}
