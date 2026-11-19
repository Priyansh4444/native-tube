//! What this app remembers that the core's library doesn't: the autoplay choice and how far each
//! video was watched (for resume and the red progress bar on thumbnails). Saved as JSON next to
//! the library, written on a core worker so the UI thread never touches the disk.

use std::collections::HashMap;
use std::path::PathBuf;

/// Watch positions kept, newest first; older ones are dropped.
const MAX_POSITIONS: usize = 400;

#[derive(Clone, Debug)]
pub struct Prefs {
    pub autoplay: bool,
    /// video id → (position s, duration s, sequence) ; the sequence orders entries for eviction.
    positions: HashMap<String, (f64, f64, u64)>,
    seq: u64,
    path: PathBuf,
}

impl Prefs {
    pub fn load() -> Prefs {
        let path = nt_core::data_dir().join("gpui-prefs.json");
        let mut p = Prefs {
            autoplay: true,
            positions: HashMap::new(),
            seq: 0,
            path,
        };
        let Ok(bytes) = std::fs::read(&p.path) else {
            return p;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return p;
        };
        if let Some(a) = v.get("autoplay").and_then(|a| a.as_bool()) {
            p.autoplay = a;
        }
        if let Some(list) = v.get("positions").and_then(|l| l.as_array()) {
            for item in list {
                let (Some(id), Some(pos), Some(dur)) = (
                    item.get(0).and_then(|x| x.as_str()),
                    item.get(1).and_then(|x| x.as_f64()),
                    item.get(2).and_then(|x| x.as_f64()),
                ) else {
                    continue;
                };
                p.seq += 1;
                p.positions.insert(id.to_string(), (pos, dur, p.seq));
            }
        }
        p
    }

    /// Fraction watched, for the progress bar on thumbnails.
    pub fn watched(&self, id: &str) -> Option<f32> {
        let (pos, dur, _) = self.positions.get(id)?;
        (*dur > 0.0 && *pos > 0.0).then(|| (pos / dur).clamp(0.0, 1.0) as f32)
    }

    /// Where to resume: more than 30 s in and more than 30 s from the end, as YouTube does.
    pub fn resume_at(&self, id: &str) -> Option<f64> {
        let (pos, dur, _) = self.positions.get(id)?;
        (*pos > 30.0 && *dur - *pos > 30.0).then_some(*pos)
    }

    pub fn set_position(&mut self, id: &str, pos: f64, dur: f64) {
        if !(pos.is_finite() && dur.is_finite()) || dur <= 0.0 {
            return;
        }
        self.seq += 1;
        self.positions
            .insert(id.to_string(), (pos.max(0.0), dur, self.seq));
        if self.positions.len() > MAX_POSITIONS {
            let mut all: Vec<(u64, String)> = self
                .positions
                .iter()
                .map(|(k, v)| (v.2, k.clone()))
                .collect();
            all.sort_unstable();
            for (_, k) in all.into_iter().take(self.positions.len() - MAX_POSITIONS) {
                self.positions.remove(&k);
            }
        }
    }

    pub fn save(&self) {
        let mut list: Vec<(&String, &(f64, f64, u64))> = self.positions.iter().collect();
        list.sort_by_key(|(_, v)| v.2);
        let json = serde_json::json!({
            "autoplay": self.autoplay,
            "positions": list.iter().map(|(k, v)| serde_json::json!([k, v.0, v.1])).collect::<Vec<_>>(),
        });
        let path = self.path.clone();
        nt_core::runtime().spawn_blocking(move || {
            let Ok(bytes) = serde_json::to_vec(&json) else {
                return;
            };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}
