//! The local library: subscriptions, watch history and Watch Later, saved as
//! one JSON file. All three apps share it unless `NT_DATA_DIR` points elsewhere.
//!
//! Reads are from memory. Writes save on a core worker thread, never on the
//! caller's thread, so a UI can call these from event handlers.

use crate::model::{ChannelRef, Video};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastPlayed {
    pub id: String,
    pub position: f64,
    pub title: String,
}

#[derive(Default, Serialize, Deserialize)]
struct StoredLibrary {
    #[serde(flatten)]
    data: LibraryData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_played: Option<LastPlayed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    queue: Vec<Video>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    hidden_videos: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    hidden_channels: std::collections::BTreeSet<String>,
}

pub struct Library {
    data: Arc<Mutex<StoredLibrary>>,
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
        let fresh = || StoredLibrary {
            data: LibraryData {
                volume: default_volume(),
                ..Default::default()
            },
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
            data: Arc::new(Mutex::new(data)),
            path,
        }
    })
}

impl Library {
    fn lock(&self) -> MutexGuard<'_, StoredLibrary> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of everything, for views that render the lists.
    pub fn snapshot(&self) -> LibraryData {
        self.lock().data.clone()
    }

    pub fn subscriptions(&self) -> Vec<ChannelRef> {
        self.lock().data.subscriptions.clone()
    }

    pub fn is_subscribed(&self, channel_id: &str) -> bool {
        self.lock()
            .data
            .subscriptions
            .iter()
            .any(|c| c.id == channel_id)
    }

    /// Subscribe or unsubscribe. Returns whether the channel is now subscribed.
    pub fn toggle_subscription(&self, channel: &ChannelRef) -> bool {
        let now = {
            let mut d = self.lock();
            if let Some(pos) = d.data.subscriptions.iter().position(|c| c.id == channel.id) {
                d.data.subscriptions.remove(pos);
                false
            } else {
                d.data.subscriptions.push(channel.clone());
                d.data.subscriptions.sort_by_key(|c| c.name.to_lowercase());
                true
            }
        };
        self.save();
        now
    }

    pub fn history(&self) -> Vec<Video> {
        self.lock().data.history.clone()
    }

    /// Put a video at the top of the history.
    pub fn record_watch(&self, video: &Video) {
        {
            let mut d = self.lock();
            d.data.history.retain(|v| v.id != video.id);
            d.data.history.insert(0, video.clone());
            d.data.history.truncate(HISTORY_LIMIT);
        }
        self.save();
    }

    pub fn clear_history(&self) {
        self.lock().data.history.clear();
        self.save();
    }

    pub fn watch_later(&self) -> Vec<Video> {
        self.lock().data.watch_later.clone()
    }

    pub fn is_saved(&self, video_id: &str) -> bool {
        self.lock()
            .data
            .watch_later
            .iter()
            .any(|v| v.id == video_id)
    }

    /// Add to or remove from Watch Later. Returns whether it is now saved.
    pub fn toggle_watch_later(&self, video: &Video) -> bool {
        let now = {
            let mut d = self.lock();
            if let Some(pos) = d.data.watch_later.iter().position(|v| v.id == video.id) {
                d.data.watch_later.remove(pos);
                false
            } else {
                d.data.watch_later.insert(0, video.clone());
                true
            }
        };
        self.save();
        now
    }

    pub(crate) fn volume(&self) -> f64 {
        self.lock().data.volume
    }

    pub fn set_volume(&self, volume: f64) {
        self.lock().data.volume = volume.clamp(0.0, 100.0);
        self.save();
    }

    pub fn queue(&self) -> Vec<Video> {
        self.lock().queue.clone()
    }

    pub fn enqueue(&self, video: &Video) {
        let mut data = self.lock();
        if data.queue.iter().any(|v| v.id == video.id) {
            return;
        }
        data.queue.push(video.clone());
        drop(data);
        self.save();
    }

    pub fn remove_queued(&self, video_id: &str) {
        self.lock().queue.retain(|v| v.id != video_id);
        self.save();
    }

    pub fn pop_queued(&self) -> Option<Video> {
        let video = {
            let mut data = self.lock();
            if data.queue.is_empty() {
                return None;
            }
            data.queue.remove(0)
        };
        self.save();
        Some(video)
    }

    pub fn set_video_hidden(&self, video_id: &str, hidden: bool) {
        let mut data = self.lock();
        let changed = if hidden {
            data.hidden_videos.insert(video_id.to_owned())
        } else {
            data.hidden_videos.remove(video_id)
        };
        drop(data);
        if changed {
            self.save();
        }
    }

    pub fn set_channel_hidden(&self, channel_id: &str, hidden: bool) {
        let mut data = self.lock();
        let changed = if hidden {
            data.hidden_channels.insert(channel_id.to_owned())
        } else {
            data.hidden_channels.remove(channel_id)
        };
        drop(data);
        if changed {
            self.save();
        }
    }

    pub fn is_hidden(&self, video: &Video) -> bool {
        self.lock().is_hidden(video)
    }

    pub(crate) fn filter_videos(&self, items: &mut Vec<Video>) {
        let data = self.lock();
        items.retain(|v| !data.is_hidden(v));
    }

    pub(crate) fn filter_search(&self, search: &mut crate::SearchPage) {
        let data = self.lock();
        search.results.items.retain(|v| !data.is_hidden(v));
        search.transcripts.items.retain(|v| !data.is_hidden(v));
        data.filter_entries(&mut search.entries);
    }

    pub fn last_played(&self) -> Option<LastPlayed> {
        self.lock().last_played.clone()
    }

    pub(crate) fn remember(&self, played: LastPlayed) {
        self.lock().last_played = Some(played);
        self.save();
    }

    /// Write the library to disk now and return when it is written (a few KB). Saves otherwise
    /// happen on a worker; call this before exit (`Player::shutdown` does) so none is lost.
    pub fn flush(&self) {
        write_current(&self.path, &self.data);
    }

    /// Write the latest state on a worker, so delayed saves cannot restore old data.
    fn save(&self) {
        let (data, path) = (self.data.clone(), self.path.clone());
        match crate::client::try_runtime() {
            Ok(rt) => drop(rt.spawn_blocking(move || write_current(&path, &data))),
            // No worker to hand the save to: write it here rather than lose it.
            Err(_) => write_current(&path, &data),
        }
    }
}

impl StoredLibrary {
    fn is_hidden(&self, v: &Video) -> bool {
        self.hidden_videos.contains(&v.id)
            || v.channel_id
                .as_ref()
                .is_some_and(|id| self.hidden_channels.contains(id))
    }

    fn filter_entries(&self, entries: &mut Vec<crate::SearchResult>) {
        use crate::SearchResult;
        entries.retain_mut(|entry| match entry {
            SearchResult::Video(v) => !self.is_hidden(v),
            SearchResult::Channel(c) => !self.hidden_channels.contains(&c.channel.id),
            SearchResult::Shelf(s) => {
                self.filter_entries(&mut s.items);
                !s.items.is_empty()
            }
            SearchResult::Playlist(_) => true,
        });
    }
}

fn write_current(path: &std::path::Path, data: &Mutex<StoredLibrary>) {
    static SAVING: Mutex<()> = Mutex::new(());
    let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(json) = serde_json::to_vec_pretty(&*data.lock().unwrap_or_else(|e| e.into_inner()))
    else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_writes_the_resume_point_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        let library = Library {
            data: Arc::new(Mutex::new(StoredLibrary::default())),
            path: path.clone(),
        };
        let checkpoint = LastPlayed {
            id: "abcdefghijk".into(),
            position: 42.5,
            title: "Resume me".into(),
        };
        library.lock().last_played = Some(checkpoint.clone());
        library.flush();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["last_played"]["id"], "abcdefghijk");
        assert_eq!(written["last_played"]["position"], 42.5);
        // Flushing again after a change writes the change, synchronously.
        library.remember(LastPlayed {
            position: 50.0,
            ..checkpoint
        });
        library.flush();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["last_played"]["position"], 50.0);
    }

    #[test]
    fn queue_and_hide_lists_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library {
            data: Arc::new(Mutex::new(StoredLibrary::default())),
            path: dir.path().join("library.json"),
        };
        let video = Video {
            id: "test".into(),
            channel_id: Some("channel".into()),
            ..Video::default()
        };
        library.enqueue(&video);
        library.enqueue(&video);
        library.set_channel_hidden("channel", true);
        library.set_video_hidden("other", true);
        library.flush();
        let file: StoredLibrary =
            serde_json::from_slice(&std::fs::read(&library.path).unwrap()).unwrap();
        assert_eq!(file.queue, vec![video.clone()]);
        assert!(file.is_hidden(&video));
        assert!(file.hidden_videos.contains("other"));
        assert_eq!(library.pop_queued(), Some(video));
        assert!(library.pop_queued().is_none());
    }

    #[test]
    fn last_played_round_trips_in_legacy_library() {
        let old = r#"{"volume":80,"history":[],"subscriptions":[],"watch_later":[]}"#;
        let mut config: StoredLibrary = serde_json::from_str(old).unwrap();
        assert_eq!(config.last_played, None);
        let played = LastPlayed {
            id: "5C_HPTJg5ek".into(),
            position: 18.25,
            title: "Rust in 100 Seconds".into(),
        };
        config.last_played = Some(played.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        write_current(&path, &Mutex::new(config));
        let recovered: StoredLibrary =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(recovered.last_played, Some(played));
        assert_eq!(recovered.data.volume, 80.0);
    }
}
