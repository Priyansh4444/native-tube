//! Things that happen off the UI thread and must reach `update`: decoded images and player
//! wake-ups (one channel, read by a subscription), plus the autopilot's command stream.

use crate::app::Message;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
use nt_core::RgbaImage;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub enum Ext {
    Image(String, u32, nt_core::Result<Arc<RgbaImage>>),
    Player,
}

type Chan = (mpsc::UnboundedSender<Ext>, Mutex<Option<mpsc::UnboundedReceiver<Ext>>>);

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

/// Set while a player wake-up is queued, so a burst of frames becomes one message.
static PLAYER_PENDING: AtomicBool = AtomicBool::new(false);

pub fn player_waker() -> impl Fn() + Send + Sync + 'static {
    || {
        if !PLAYER_PENDING.swap(true, Ordering::AcqRel) {
            send(Ext::Player);
        }
    }
}

/// Call before reading the player's state, so a wake that lands meanwhile queues again.
pub fn player_taken() {
    PLAYER_PENDING.store(false, Ordering::Release);
}

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
