//! The app's single `nt_core::Player`, and how its frames reach the screen.
//!
//! mpv renders BGRA into a CPU buffer on the core's render thread and runs our waker,
//! which pings a bounded(1) channel (so a burst of wakes collapses into one). A
//! foreground task drains it and notifies the view only when the frame generation or
//! a displayed part of the playback state changed. On the next frame the newest
//! picture is copied once into a `RenderImage`, GPUI uploads it into its texture atlas
//! when it is painted, and the previous picture's atlas tile is freed with
//! `Window::drop_image`, so exactly one video tile is alive at a time.

use crate::app::NtApp;
use gpui::{Context, RenderImage, Window};
use nt_core::{PixelFormat, PlaybackState, Player, Status};
use std::sync::Arc;

#[derive(Default)]
pub struct VideoState {
    pub player: Option<Player>,
    pub error: Option<String>,
    pub state: PlaybackState,
    pub image: Option<Arc<RenderImage>>,
    shown_gen: u64,
    seen_gen: u64,
    /// Emit `playback_started` when the next picture is drawn.
    pub started_pending: bool,
    target: (u32, u32),
}

/// The parts of the playback state the UI shows; a change in any of them needs a frame.
fn shown(s: &PlaybackState) -> (Status, bool, bool, bool, i64, i64, i64, i64) {
    (
        s.status.clone(),
        s.paused,
        s.buffering,
        s.muted,
        s.volume.round() as i64,
        s.duration.round() as i64,
        (s.position * 4.0) as i64,
        (s.buffered_ahead * 2.0) as i64,
    )
}

impl VideoState {
    /// The player, created on first use.
    pub fn ensure(&mut self, cx: &mut Context<NtApp>) -> Option<&Player> {
        if self.player.is_none() && self.error.is_none() {
            let (tx, rx) = async_channel::bounded::<()>(1);
            match Player::new(PixelFormat::Bgra, move || {
                let _ = tx.try_send(());
            }) {
                Ok(p) => {
                    self.player = Some(p);
                    cx.spawn(async move |this, cx| {
                        while rx.recv().await.is_ok() {
                            if this.update(cx, |app, cx| app.on_player_wake(cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.player.as_ref()
    }

    pub fn load(&mut self, id: &str, cx: &mut Context<NtApp>) {
        if let Some(p) = self.ensure(cx) {
            match p.load(id) {
                Ok(()) => self.started_pending = true,
                Err(e) => self.state.status = Status::Failed(e.to_string()),
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(p) = &self.player {
            p.stop();
        }
        self.started_pending = false;
    }

    /// Called on every player wake. Returns whether the UI must redraw.
    pub fn poll(&mut self) -> bool {
        let Some(p) = &self.player else { return false };
        let generation = p.frame_generation();
        let state = p.state();
        let changed = generation != self.seen_gen || shown(&state) != shown(&self.state);
        self.seen_gen = generation;
        self.state = state;
        changed
    }

    /// Before painting the player: take the newest picture if it changed. Returns whether
    /// a new picture will be drawn this frame.
    pub fn upload(&mut self, window: &mut Window) -> bool {
        let Some(p) = &self.player else { return false };
        let generation = p.frame_generation();
        if generation == self.shown_gen && (self.image.is_some() || generation == 0) {
            return false;
        }
        self.shown_gen = generation;
        let Some(frame) = p.frame() else {
            if let Some(old) = self.image.take() {
                let _ = window.drop_image(old);
            }
            return false;
        };
        let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels().to_vec()) else {
            return false;
        };
        let new = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)]));
        if let Some(old) = self.image.replace(new) {
            let _ = window.drop_image(old);
        }
        true
    }

    /// Render at the size the video is drawn, in physical pixels.
    pub fn set_target(&mut self, w: u32, h: u32) {
        if self.target != (w, h) {
            self.target = (w, h);
            if let Some(p) = &self.player {
                p.set_target_size(w, h);
            }
        }
    }
}
