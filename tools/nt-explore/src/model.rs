use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub t_ms: u64,
    pub page: String,
    pub query: Option<String>,
    pub results_query: Option<String>,
    pub topic: Option<String>,
    pub scroll_y: f64,
    pub max_scroll: f64,
    pub items: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub rail_selected: Option<String>,
    pub menu: Option<String>,
    pub focus: Option<String>,
    pub video: Option<Video>,
    pub fullscreen: bool,
    pub theatre: bool,
    pub back_depth: usize,
    pub forward_depth: usize,
    pub targets: Vec<Target>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub status: String,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub muted: bool,
    pub first_frame: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub kind: String,
    pub rect: [f64; 4],
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetId {
    pub kind: String,
    pub label: String,
    pub occurrence: usize,
}

impl TargetId {
    pub fn from_visible(targets: &[Target], index: usize) -> Self {
        let target = &targets[index];
        Self {
            kind: target.kind.clone(),
            label: target.label.clone(),
            occurrence: targets[..index]
                .iter()
                .filter(|other| other.kind == target.kind && other.label == target.label)
                .count(),
        }
    }

    pub fn find<'a>(&self, state: &'a State) -> Option<&'a Target> {
        state
            .targets
            .iter()
            .filter(|target| target.kind == self.kind && target.label == self.label)
            .nth(self.occurrence)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Click { target: TargetId, right: bool },
    Wheel { point: [i32; 2], ticks: i32 },
    Fling { point: [i32; 2], pixels: i32 },
    Search { target: TargetId, text: String },
    Key { key: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    Process,
    Panic,
    ScrollBounds,
    RailSelection,
    Pss,
    BackRestoration,
    WatchFirstFrame,
    EscapeDismissesMenu,
    SearchResults,
    LoadingEnds,
    Probe,
    Driver,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Violation {
    pub property: Property,
    pub detail: String,
    pub screenshot: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    State { elapsed_ms: u64, state: Box<State> },
    Action { elapsed_ms: u64, action: Action },
    Health { elapsed_ms: u64, pss_kib: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Trace {
    pub version: u32,
    pub app: String,
    pub seed: u64,
    pub events: Vec<Event>,
    pub violation: Option<Violation>,
}

impl Trace {
    pub fn actions(&self) -> Vec<Action> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Action { action, .. } => Some(action.clone()),
                _ => None,
            })
            .collect()
    }
}
