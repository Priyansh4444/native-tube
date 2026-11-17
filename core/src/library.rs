//! The local library: subscriptions, watch history and Watch Later, saved as
//! one JSON file. All three apps share it unless `NT_DATA_DIR` points elsewhere.
//!
//! Reads are from memory. Writes save on a core worker thread, never on the
//! caller's thread, so a UI can call these from event handlers.

use crate::model::{ChannelRef, Video};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const HISTORY_LIMIT: usize = 500;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibraryData {
    pub subscriptions: Vec<ChannelRef>,
    /// Newest first.
    pub history: Vec<Video>,
    /// Newest first.
    pub watch_later: Vec<Video>,
    /// Last playback volume, 0-100.
    #[serde(default = "default_volume")]
    pub volume: f64,
}

fn default_volume() -> f64 {
    80.0
}

pub struct Library {
    data: Mutex<LibraryData>,
    path: PathBuf,
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NT_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("native-tube")
}

/// The process-wide library, loaded on first use.
pub fn library() -> &'static Library {
    static L: OnceLock<Library> = OnceLock::new();
    L.get_or_init(|| {
        let path = data_dir().join("library.json");
        let fresh = || LibraryData {
            volume: default_volume(),
            ..Default::default()
        };
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                // Never let an unreadable library be replaced by an empty one: keep it aside.
                let kept = path.with_extension(format!("json.unreadable-{}", std::process::id()));
                let _ = std::fs::rename(&path, &kept);
                eprintln!(
                    "nt-core: library.json could not be read ({e}); kept as {}",
                    kept.display()
                );
                fresh()
            }),
            Err(_) => fresh(),
        };
        Library {
            data: Mutex::new(data),
            path,
        }
    })
}

impl Library {
    fn lock(&self) -> MutexGuard<'_, LibraryData> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of everything, for views that render the lists.
    pub fn snapshot(&self) -> LibraryData {
        self.lock().clone()
    }

    pub fn subscriptions(&self) -> Vec<ChannelRef> {
        self.lock().subscriptions.clone()
    }

    pub fn is_subscribed(&self, channel_id: &str) -> bool {
        self.lock().subscriptions.iter().any(|c| c.id == channel_id)
    }

    /// Subscribe or unsubscribe. Returns whether the channel is now subscribed.
    pub fn toggle_subscription(&self, channel: &ChannelRef) -> bool {
        let now = {
            let mut d = self.lock();
            if let Some(pos) = d.subscriptions.iter().position(|c| c.id == channel.id) {
                d.subscriptions.remove(pos);
                false
            } else {
                d.subscriptions.push(channel.clone());
                d.subscriptions.sort_by_key(|c| c.name.to_lowercase());
                true
            }
        };
        self.save();
        now
    }

    pub fn history(&self) -> Vec<Video> {
        self.lock().history.clone()
    }

    /// Put a video at the top of the history.
    pub fn record_watch(&self, video: &Video) {
        {
            let mut d = self.lock();
            d.history.retain(|v| v.id != video.id);
            d.history.insert(0, video.clone());
            d.history.truncate(HISTORY_LIMIT);
        }
        self.save();
    }

    pub fn clear_history(&self) {
        self.lock().history.clear();
        self.save();
    }

    pub fn watch_later(&self) -> Vec<Video> {
        self.lock().watch_later.clone()
    }

    pub fn is_saved(&self, video_id: &str) -> bool {
        self.lock().watch_later.iter().any(|v| v.id == video_id)
    }

    /// Add to or remove from Watch Later. Returns whether it is now saved.
    pub fn toggle_watch_later(&self, video: &Video) -> bool {
        let now = {
            let mut d = self.lock();
            if let Some(pos) = d.watch_later.iter().position(|v| v.id == video.id) {
                d.watch_later.remove(pos);
                false
            } else {
                d.watch_later.insert(0, video.clone());
                true
            }
        };
        self.save();
        now
    }

    pub fn volume(&self) -> f64 {
        self.lock().volume
    }

    pub fn set_volume(&self, volume: f64) {
        self.lock().volume = volume.clamp(0.0, 100.0);
        self.save();
    }

    /// Write the file on a worker: a temporary file, then a rename, so a crash never leaves half a file.
    fn save(&self) {
        let (data, path) = (self.snapshot(), self.path.clone());
        crate::client::runtime().spawn_blocking(move || {
            let Ok(json) = serde_json::to_vec_pretty(&data) else {
                return;
            };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // One writer at a time, each with its own temp file: the last save wins whole.
            static SAVING: Mutex<()> = Mutex::new(());
            let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}
