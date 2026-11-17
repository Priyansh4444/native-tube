//! MPRIS: the desktop's view of the player (Linux).
//!
//! Publishing the player as `org.mpris.MediaPlayer2.nativetube.instance<pid>` gives the viewer:
//! - media keys and the desktop's "now playing" controls (play, pause, seek);
//! - no screen lock in the middle of a video on desktops whose idle manager holds off while an
//!   MPRIS player is playing (Caelestia, KDE, many others);
//! - title, channel, length and artwork in the desktop's media widget.
//!
//! Started by `Player::new`. Off when `NT_NO_MPRIS` is set (all test and benchmark runs), so a
//! test never shows up in the owner's panel or answers their media keys.

use crate::player::{Remote, Status};
use std::collections::HashMap;
use zbus::{ObjectPath, OwnedVariant as OwnedValue, Variant as Value};

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "NativeTube".to_string()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct PlayerIface {
    remote: Remote,
}

fn status_text(status: &Status, paused: bool) -> &'static str {
    match status {
        Status::Ready if !paused => "Playing",
        Status::Ready | Status::Ended => "Paused",
        // Loading after a recovery or a quality change is still "playing" to the viewer.
        Status::Loading => "Playing",
        Status::Idle | Status::Failed(_) => "Stopped",
    }
}

fn track_path(source: &str) -> String {
    let id: String = source
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("/org/nativetube/track/v{id}")
}

fn owned(v: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(v).ok()
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) {}
    fn previous(&self) {}
    fn pause(&self) {
        self.remote.set_paused(true);
    }
    fn play(&self) {
        self.remote.set_paused(false);
    }
    fn play_pause(&self) {
        let paused = self.remote.state().paused;
        self.remote.set_paused(!paused);
    }
    fn stop(&self) {
        self.remote.set_paused(true);
    }
    /// `offset` in microseconds.
    fn seek(&self, offset: i64) {
        self.remote.seek_by(offset as f64 / 1e6);
    }
    fn set_position(&self, _track: ObjectPath<'_>, position: i64) {
        self.remote.seek_to(position as f64 / 1e6);
    }
    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        let s = self.remote.state();
        status_text(&s.status, s.paused).to_string()
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        "None".to_string()
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        let s = self.remote.state().speed;
        if s > 0.0 { s } else { 1.0 }
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let s = self.remote.state();
        let mut m = HashMap::new();
        let Some(source) = s.source.as_deref() else {
            return m;
        };
        if let Ok(path) = ObjectPath::try_from(track_path(source))
            && let Some(v) = owned(Value::from(path))
        {
            m.insert("mpris:trackid".to_string(), v);
        }
        if s.duration > 0.0
            && let Some(v) = owned(Value::from((s.duration * 1e6) as i64))
        {
            m.insert("mpris:length".to_string(), v);
        }
        if !s.title.is_empty()
            && let Some(v) = owned(Value::from(s.title.clone()))
        {
            m.insert("xesam:title".to_string(), v);
        }
        if !s.channel.is_empty()
            && let Some(v) = owned(Value::from(vec![s.channel.clone()]))
        {
            m.insert("xesam:artist".to_string(), v);
        }
        if source.len() == 11
            && let Some(v) = owned(Value::from(format!(
                "https://i.ytimg.com/vi/{source}/hqdefault.jpg"
            )))
        {
            m.insert("mpris:artUrl".to_string(), v);
            if let Some(v) = owned(Value::from(format!(
                "https://www.youtube.com/watch?v={source}"
            ))) {
                m.insert("xesam:url".to_string(), v);
            }
        }
        m
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        let s = self.remote.state();
        if s.muted { 0.0 } else { s.volume / 100.0 }
    }
    /// Read on demand; MPRIS says not to signal it.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        (self.remote.state().position * 1e6) as i64
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        0.25
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        2.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        !self.remote.state().is_live
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Publish the player on the session bus, if there is one. Never fails the caller.
pub(crate) fn start(remote: Remote) {
    if std::env::var_os("NT_NO_MPRIS").is_some() {
        return;
    }
    crate::client::runtime().spawn(async move {
        if let Err(e) = run(remote).await {
            eprintln!("nt-core: MPRIS unavailable ({e})");
        }
    });
}

async fn run(remote: Remote) -> zbus::Result<()> {
    let name = format!(
        "org.mpris.MediaPlayer2.nativetube.instance{}",
        std::process::id()
    );
    let conn = zbus::connection::Builder::session()
        .name(name)
        .serve_at("/org/mpris/MediaPlayer2", Root)
        .serve_at(
            "/org/mpris/MediaPlayer2",
            PlayerIface {
                remote: remote.clone(),
            },
        )
        .build()
        .await?;
    let iface = conn
        .object_server()
        .interface::<_, PlayerIface>("/org/mpris/MediaPlayer2")
        .await?;
    // Signal only what changed, at most every 250 ms (position changes each second do not count).
    let mut last = (String::new(), None, String::new(), 0i64, -1.0f64, true);
    loop {
        remote.changed().await;
        if remote.is_gone() {
            return Ok(());
        }
        let s = remote.state();
        let now = (
            status_text(&s.status, s.paused).to_string(),
            s.source.clone(),
            s.title.clone(),
            (s.duration * 1e6) as i64,
            if s.muted { 0.0 } else { s.volume },
            s.is_live,
        );
        let i = iface.get().await;
        let emitter = iface.signal_emitter();
        if now.0 != last.0 {
            i.playback_status_changed(emitter).await?;
        }
        if now.1 != last.1 || now.2 != last.2 || now.3 != last.3 {
            i.metadata_changed(emitter).await?;
        }
        if now.4 != last.4 {
            i.volume_changed(emitter).await?;
        }
        if now.5 != last.5 {
            i.can_seek_changed(emitter).await?;
        }
        drop(i);
        last = now;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}
