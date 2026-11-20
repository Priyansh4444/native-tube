//! One reusable, muted mpv preview, with each hover owning its own Pixels buffer.

use nt_core::{PixelFormat, Player};
use rune::{Pixels, Tx};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

const DELAY: Duration = Duration::from_millis(850);

#[derive(Default)]
pub struct Preview {
    pending: Option<(String, f64, u32)>,
    active: Option<(String, Pixels)>,
}

type Request = Option<(String, Pixels, u32)>;
type Output = Option<(String, Pixels, u64)>;
static PLAYER: OnceLock<Arc<Player>> = OnceLock::new();
static OUTPUT: Mutex<Output> = Mutex::new(None);
static COMMAND: OnceLock<mpsc::Sender<Request>> = OnceLock::new();

fn output() -> std::sync::MutexGuard<'static, Output> {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner())
}

fn wake() {
    let Some(player) = PLAYER.get() else { return };
    let Some(frame) = player.frame() else { return };
    let mut output = output();
    if let Some((id, pixels, generation)) = output.as_mut()
        && frame.generation > *generation
        && player.state().source.as_deref() == Some(id)
    {
        pixels.set(frame.width, frame.height, frame.pixels().to_vec());
        *generation = frame.generation;
    }
}

fn commands(receiver: mpsc::Receiver<Request>) {
    while let Ok(first) = receiver.recv() {
        let latest = receiver.try_iter().last().unwrap_or(first);
        *output() = None;
        let Some((id, pixels, width)) = latest else {
            if let Some(player) = PLAYER.get() {
                player.stop();
            }
            continue;
        };
        if PLAYER.get().is_none() {
            match Player::new_preview(PixelFormat::Rgba, wake) {
                Ok(player) => {
                    let _ = PLAYER.set(Arc::new(player));
                }
                Err(error) => {
                    eprintln!("nt-rune: preview unavailable: {error}");
                    continue;
                }
            }
        }
        let Some(player) = PLAYER.get() else { continue };
        player.set_target_size(width.min(640), (width.min(640) * 9 / 16).max(1));
        if player.load(&id).is_ok() {
            *output() = Some((id, pixels, 0));
            wake();
        }
    }
}

fn request(command: Request) {
    let sender = COMMAND.get_or_init(|| {
        let (send, receive) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("nt-preview-cmd".into())
            .spawn(move || commands(receive));
        send
    });
    let _ = sender.send(command);
}

impl Preview {
    pub fn hover(&mut self, id: &str, width: u32, inside: bool, tx: &mut Tx) {
        if !inside {
            if self.pending.as_ref().is_some_and(|(v, _, _)| v == id)
                || self.active.as_ref().is_some_and(|(v, _)| v == id)
            {
                self.cancel();
            }
            return;
        }
        if self.pending.as_ref().is_some_and(|(v, _, _)| v == id)
            || self.active.as_ref().is_some_and(|(v, _)| v == id)
        {
            return;
        }
        self.cancel();
        self.pending = Some((
            id.to_string(),
            crate::scroller::now() + DELAY.as_secs_f64(),
            width,
        ));
        tx.wake_after(DELAY);
    }

    pub fn cancel(&mut self) -> bool {
        self.pending = None;
        let active = self.active.take().is_some();
        if active {
            request(None);
        }
        active
    }

    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let Some((_, due, _)) = &self.pending else {
            return false;
        };
        if *due > now {
            tx.wake_after(Duration::from_secs_f64(*due - now));
            return false;
        }
        if let Some((id, _, width)) = self.pending.take() {
            let pixels = Pixels::new();
            request(Some((id.clone(), pixels.clone(), width)));
            self.active = Some((id, pixels));
            return true;
        }
        false
    }

    pub fn pixels(&self, id: &str) -> Option<&Pixels> {
        let (active, pixels) = self.active.as_ref()?;
        (active == id).then_some(pixels)
    }
}
