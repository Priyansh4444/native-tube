//! Scripted sessions for benchmarks and end-to-end checks. Every frontend
//! obeys the same [`Command`]s, so one scenario file drives all three apps
//! through the same clicks, scrolls and waits.
//!
//! `NT_AUTOPILOT=<scenario.json>` turns it on. The scenario is a JSON array of
//! steps (see [`Step`]). When it ends, the metrics report is written to
//! `NT_REPORT` (default `<data dir>/report-<app>.json`) and the app is told to quit.
//! `NT_SNAPSHOT_OUTPUT=<wayland output>` plus `NT_SNAPSHOT_DIR=<dir>` makes
//! `snapshot` steps save `grim` screenshots of that output.

use crate::metrics;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// Something the frontend must do, exactly as if the user did it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Go to the home feed (optionally a topic id from `TOPICS`). Mark `home_loaded` when shown.
    Home {
        #[serde(default)]
        topic: Option<String>,
    },
    /// Type `query` into search and submit. Mark `search_loaded` when results show.
    Search {
        query: String,
    },
    /// Open the video at `index` in the list on screen (home, search, channel, or the watch
    /// page's related list). Mark `watch_loaded`, then `playback_started` on the first picture.
    OpenVideo {
        index: usize,
    },
    /// Open a specific video through the normal Watch path.
    OpenVideoId {
        id: String,
    },
    /// Open the channel of the video on the watch page. Mark `channel_loaded`.
    OpenChannel,
    /// Go back one page.
    Back,
    /// Mouse-wheel notches into the main scroll view (positive scrolls down), through the same
    /// code path real wheel events take.
    Scroll {
        ticks: f64,
    },
    /// One precise (touchpad) scroll delta in logical pixels, through the real touchpad path.
    ScrollPixels {
        px: f64,
    },
    /// A touchpad fling at this velocity (logical px/s), through the real fling path.
    Fling {
        velocity: f64,
    },
    /// Jump the main scroll view to the top.
    ScrollToTop,
    TogglePause,
    /// Load the next page of comments on the watch page. Mark `comments_loaded`.
    LoadMoreComments,
    /// Exit the app cleanly.
    Quit,
}

/// One scenario step.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Step {
    /// Start a named measurement phase.
    Phase {
        phase: String,
    },
    /// Wait for a mark issued after the previous command (default timeout 30 s; on timeout the
    /// step is recorded as failed and the scenario goes on).
    WaitMark {
        wait_mark: String,
        #[serde(default)]
        timeout: Option<f64>,
    },
    Sleep {
        sleep: f64,
    },
    Snapshot {
        snapshot: String,
    },
    /// Send `do` `repeat` times, `every` seconds apart.
    Repeat {
        repeat: u32,
        every: f64,
        #[serde(rename = "do")]
        command: Command,
    },
    /// `{ "do": { "cmd": … } }`
    Do {
        #[serde(rename = "do")]
        command: Command,
    },
    /// A bare `{ "cmd": … }`.
    Bare(Command),
}

type Waker = Box<dyn Fn() + Send + Sync>;

struct Pilot {
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Command>>,
    waker: Mutex<Option<Waker>>,
}

static PILOT: OnceLock<Option<Pilot>> = OnceLock::new();

/// Whether a scenario is running.
pub fn enabled() -> bool {
    PILOT.get().is_some_and(Option::is_some)
}

/// Start the scenario named by `NT_AUTOPILOT`, if any. Call once, after `metrics::init`.
/// The frontend then takes commands with [`next_command`] (async) or [`try_next`] + [`set_waker`].
pub fn start() {
    PILOT.get_or_init(|| {
        let path = std::env::var_os("NT_AUTOPILOT")?;
        let steps: Vec<Step> = match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                // A benchmark must never run "successfully" without its scenario.
                eprintln!("NT_AUTOPILOT: can't read {}: {e}", path.to_string_lossy());
                std::process::exit(2);
            }
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = std::thread::Builder::new()
            .name("nt-autopilot".into())
            .spawn(move || run(steps, tx));
        Some(Pilot {
            rx: tokio::sync::Mutex::new(rx),
            waker: Mutex::new(None),
        })
    });
}

/// The next command, waiting for it. `None` when the scenario has ended or none is running
/// (in which case it never resolves, so a subscription built on it just stays idle).
pub async fn next_command() -> Option<Command> {
    match PILOT.get().and_then(Option::as_ref) {
        Some(p) => p.rx.lock().await.recv().await,
        None => std::future::pending().await,
    }
}

/// For frontends that poll: run `waker` whenever a command is ready, then drain with [`try_next`].
pub fn set_waker(waker: impl Fn() + Send + Sync + 'static) {
    if let Some(p) = PILOT.get().and_then(Option::as_ref) {
        *p.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(waker));
    }
}

/// The next ready command, without waiting.
pub fn try_next() -> Option<Command> {
    let p = PILOT.get()?.as_ref()?;
    p.rx.try_lock().ok()?.try_recv().ok()
}

fn send(tx: &mpsc::UnboundedSender<Command>, cmd: Command) {
    metrics::command(&serde_json::to_string(&cmd).unwrap_or_default());
    let _ = tx.send(cmd);
    if let Some(p) = PILOT.get().and_then(Option::as_ref)
        && let Some(w) = p.waker.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
    {
        w();
    }
}

fn run(steps: Vec<Step>, tx: mpsc::UnboundedSender<Command>) {
    let mut after = 0;
    let mut failures: Vec<String> = Vec::new();
    for step in steps {
        match step {
            Step::Phase { phase } => metrics::phase(&phase),
            Step::WaitMark { wait_mark, timeout } => {
                let timeout = Duration::from_secs_f64(timeout.unwrap_or(30.0));
                if metrics::wait_mark(&wait_mark, after, timeout).is_none() {
                    eprintln!("NT_AUTOPILOT: timed out waiting for {wait_mark}");
                    failures.push(wait_mark);
                }
            }
            Step::Sleep { sleep } => std::thread::sleep(Duration::from_secs_f64(sleep)),
            Step::Snapshot { snapshot } => take_snapshot(&snapshot),
            Step::Repeat {
                repeat,
                every,
                command,
            } => {
                after = metrics::mark_seq();
                for _ in 0..repeat {
                    send(&tx, command.clone());
                    std::thread::sleep(Duration::from_secs_f64(every));
                }
            }
            Step::Do { command } | Step::Bare(command) => {
                after = metrics::mark_seq();
                send(&tx, command);
            }
        }
    }
    let mut report = metrics::report();
    report["failed_waits"] = serde_json::json!(failures);
    let path = std::env::var_os("NT_REPORT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            crate::library::data_dir().join(format!(
                "report-{}.json",
                report["app"].as_str().unwrap_or("app")
            ))
        });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).unwrap_or_default(),
    ) {
        Ok(()) => eprintln!("NT_REPORT {}", path.display()),
        Err(e) => eprintln!("NT_AUTOPILOT: can't write report: {e}"),
    }
    send(&tx, Command::Quit);
    // A frontend that ignores Quit still ends, so a benchmark run can't hang.
    std::thread::sleep(Duration::from_secs(5));
    std::process::exit(0);
}

fn take_snapshot(name: &str) {
    let (Some(output), Some(dir)) = (
        std::env::var_os("NT_SNAPSHOT_OUTPUT"),
        std::env::var_os("NT_SNAPSHOT_DIR"),
    ) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{name}.png"));
    let status = std::process::Command::new("grim")
        .arg("-o")
        .arg(&output)
        .arg(&file)
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        eprintln!("NT_AUTOPILOT: snapshot {name} failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_video_id_parses() {
        let command: Command =
            serde_json::from_str(r#"{"cmd":"open_video_id","id":"5C_HPTJg5ek"}"#).unwrap();
        assert!(matches!(command, Command::OpenVideoId { id } if id == "5C_HPTJg5ek"));
    }

    /// The benchmark scenario must parse, step for step.
    #[test]
    fn scenario_parses() {
        let steps: Vec<Step> =
            serde_json::from_str(include_str!("../../bench/scenario.json")).unwrap();
        assert!(steps.iter().any(|s| matches!(
            s,
            Step::Do {
                command: Command::Search { .. }
            }
        )));
        assert!(steps.iter().any(|s| matches!(s, Step::Repeat { .. })));
        let bare: Vec<Step> = serde_json::from_str(r#"[{"cmd":"back"}]"#).unwrap();
        assert!(matches!(bare[0], Step::Bare(Command::Back)));
        // A step that mixes keys, or misspells one, is an error rather than a silently different step.
        assert!(serde_json::from_str::<Vec<Step>>(r#"[{"do":{"cmd":"back"},"sleep":1}]"#).is_err());
        assert!(serde_json::from_str::<Vec<Step>>(r#"[{"wait_mark":"x","timout":3}]"#).is_err());
    }
}
