//! Small viewer preferences the core's library doesn't hold yet (see NOTES.md, Core requests):
//! autoplay on/off, theatre mode, and how far each video was watched (for resume and the red
//! progress bar on thumbnails). Saved as one JSON file in the core's data directory, written on
//! a core worker thread. Bounded: at most `MAX_POSITIONS` videos are remembered.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

const MAX_POSITIONS: usize = 500;
const FILE: &str = "iced-prefs.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default = "yes")]
    pub autoplay: bool,
    #[serde(default)]
    pub theatre: bool,
    /// (video id, seconds watched, duration), newest last.
    #[serde(default)]
    positions: VecDeque<(String, f32, f32)>,
    /// Videos marked "Not interested", newest last.
    #[serde(default)]
    hidden: VecDeque<String>,
    /// id -> (position, duration), rebuilt from `positions`; card views look up here.
    #[serde(skip)]
    index: HashMap<String, (f32, f32)>,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            autoplay: true,
            theatre: false,
            positions: VecDeque::new(),
            hidden: VecDeque::new(),
            index: HashMap::new(),
        }
    }
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read(nt_core::data_dir().join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Prefs>(&b).ok())
            .map(|mut p| {
                p.reindex();
                p
            })
            .unwrap_or_default()
    }

    fn reindex(&mut self) {
        self.index = self
            .positions
            .iter()
            .map(|(id, p, d)| (id.clone(), (*p, *d)))
            .collect();
    }

    pub fn save(&self) {
        let Ok(json) = serde_json::to_vec(self) else {
            return;
        };
        let dir = nt_core::data_dir();
        nt_core::runtime().spawn_blocking(move || {
            let _ = std::fs::create_dir_all(&dir);
            let tmp = dir.join(format!("{FILE}.tmp"));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join(FILE));
            }
        });
    }

    /// Fraction watched, for the bar under a thumbnail.
    pub fn progress(&self, id: &str) -> Option<f32> {
        self.index
            .get(id)
            .filter(|(p, d)| *d > 0.0 && *p > 1.0)
            .map(|(p, d)| (p / d).clamp(0.0, 1.0))
    }

    /// Where to resume: only past the first 30 s and before the last 30 s, as YouTube does.
    pub fn resume_at(&self, id: &str) -> f64 {
        self.index
            .get(id)
            .filter(|(p, d)| *p > 30.0 && *d > 0.0 && *p < d - 30.0)
            .map(|(p, _)| *p as f64)
            .unwrap_or(0.0)
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|v| v == id)
    }

    pub fn hide(&mut self, id: &str) {
        if !self.is_hidden(id) {
            self.hidden.push_back(id.to_string());
            while self.hidden.len() > MAX_POSITIONS {
                self.hidden.pop_front();
            }
        }
    }

    pub fn record(&mut self, id: &str, position: f64, duration: f64) {
        if id.is_empty() || !position.is_finite() || !duration.is_finite() || duration <= 0.0 {
            return;
        }
        self.positions.retain(|(v, _, _)| v != id);
        self.positions
            .push_back((id.to_string(), position.max(0.0) as f32, duration as f32));
        while self.positions.len() > MAX_POSITIONS {
            if let Some((old, _, _)) = self.positions.pop_front() {
                self.index.remove(&old);
            }
        }
        self.index
            .insert(id.to_string(), (position.max(0.0) as f32, duration as f32));
    }
}
