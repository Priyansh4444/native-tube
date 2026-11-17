//! In-process measurements for the benchmark: presented frames, named marks,
//! and a 100 ms sampler of the process's CPU time and resident memory. Split
//! into phases (named by the autopilot) so a report can say what a scroll or a
//! minute of playback cost.
//!
//! Frontends call [`init`] first thing in `main`, [`frame`] once per presented
//! frame, and [`mark`] at the standard milestones in [`marks`].

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The milestones every frontend reports, by these exact names.
pub mod marks {
    /// The first frame with real UI (not a blank window) was presented.
    pub const FIRST_FRAME: &str = "first_frame";
    /// The first page of the home feed is on screen.
    pub const HOME_LOADED: &str = "home_loaded";
    /// Search results are on screen.
    pub const SEARCH_LOADED: &str = "search_loaded";
    /// The watch page's title, channel and related list are on screen.
    pub const WATCH_LOADED: &str = "watch_loaded";
    /// The first video picture was drawn by the UI.
    pub const PLAYBACK_STARTED: &str = "playback_started";
    /// A page of comments is on screen.
    pub const COMMENTS_LOADED: &str = "comments_loaded";
    /// A channel page is on screen.
    pub const CHANNEL_LOADED: &str = "channel_loaded";
    /// Infinite scroll appended a page.
    pub const MORE_LOADED: &str = "more_loaded";
}

struct Phase {
    name: String,
    start: f64,
    end: Option<f64>,
    cpu_start: f64,
    cpu_end: f64,
    frame_start: usize,
    frame_end: usize,
    video_start: u64,
    video_end: u64,
    rss_peak: u64,
    rss_end: u64,
}

struct State {
    app: String,
    frames: Vec<f64>,
    marks: Vec<(String, f64, u64)>,
    /// (t, command as JSON) for every autopilot command, to compute input-to-result latency.
    commands: Vec<(f64, String)>,
    phases: Vec<Phase>,
    /// (t, cpu seconds, rss bytes)
    samples: Vec<(f64, f64, u64)>,
    rss_peak: u64,
    exec_to_init: Option<f64>,
}

struct Metrics {
    start: Instant,
    state: Mutex<State>,
    mark_cv: Condvar,
    seq: AtomicU64,
    video_frames: AtomicU64,
}

fn metrics() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(|| Metrics {
        start: Instant::now(),
        state: Mutex::new(State {
            app: String::new(),
            frames: Vec::with_capacity(16_384),
            marks: Vec::new(),
            commands: Vec::new(),
            phases: Vec::new(),
            samples: Vec::new(),
            rss_peak: 0,
            exec_to_init: None,
        }),
        mark_cv: Condvar::new(),
        seq: AtomicU64::new(0),
        video_frames: AtomicU64::new(0),
    })
}

fn lock() -> MutexGuard<'static, State> {
    metrics().state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Seconds since [`init`].
pub fn now() -> f64 {
    metrics().start.elapsed().as_secs_f64()
}

/// Call first thing in `main`. Starts the sampler (only when `NT_AUTOPILOT` or `NT_METRICS` is set,
/// so a normal launch pays nothing) and records how long the process took to get here.
pub fn init(app: &str) {
    let m = metrics();
    {
        let mut s = lock();
        s.app = app.to_string();
        s.exec_to_init = seconds_since_exec();
    }
    if std::env::var_os("NT_AUTOPILOT").is_some() || std::env::var_os("NT_METRICS").is_some() {
        let _ = std::thread::Builder::new()
            .name("nt-metrics".into())
            .spawn(|| {
                loop {
                    let (cpu, rss) = (cpu_seconds(), rss_bytes());
                    {
                        let mut s = lock();
                        let t = now();
                        s.samples.push((t, cpu, rss));
                        s.rss_peak = s.rss_peak.max(rss);
                        if let Some(p) = s.phases.last_mut()
                            && p.end.is_none()
                        {
                            p.rss_peak = p.rss_peak.max(rss);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
    }
    let _ = m;
}

/// Call once per presented frame.
pub fn frame() {
    let t = now();
    let mut s = lock();
    let first = s.frames.is_empty();
    s.frames.push(t);
    drop(s);
    if first {
        trace("frame", t);
    }
}

/// Called by the player for every picture it renders.
pub(crate) fn video_frame() {
    metrics().video_frames.fetch_add(1, Ordering::Relaxed);
}

/// Record a milestone (see [`marks`]). Only the first of each name is printed.
pub fn mark(name: &str) {
    let t = now();
    let seq = metrics().seq.fetch_add(1, Ordering::AcqRel) + 1;
    let mut s = lock();
    let first = !s.marks.iter().any(|(n, _, _)| n == name);
    s.marks.push((name.to_string(), t, seq));
    drop(s);
    metrics().mark_cv.notify_all();
    if first {
        trace(name, t);
    }
}

fn trace(name: &str, t: f64) {
    if std::env::var_os("NT_TRACE").is_some() || std::env::var_os("NT_AUTOPILOT").is_some() {
        eprintln!("NT_MARK {name} {:.1}", t * 1000.0);
    }
}

/// Record that the autopilot sent a command now.
pub(crate) fn command(json: &str) {
    let t = now();
    lock().commands.push((t, json.to_string()));
}

/// The sequence number of the newest mark; [`wait_mark`] waits for marks after a given one.
pub fn mark_seq() -> u64 {
    metrics().seq.load(Ordering::Acquire)
}

/// Block until `name` is marked with a sequence number above `after`. Returns its time, or `None` on timeout.
pub fn wait_mark(name: &str, after: u64, timeout: Duration) -> Option<f64> {
    let deadline = Instant::now() + timeout;
    let mut s = lock();
    loop {
        if let Some((_, t, _)) = s.marks.iter().find(|(n, _, seq)| n == name && *seq > after) {
            return Some(*t);
        }
        let left = deadline.checked_duration_since(Instant::now())?;
        s = metrics()
            .mark_cv
            .wait_timeout(s, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// Start a named phase (ending the previous one).
pub fn phase(name: &str) {
    let (t, cpu, rss) = (now(), cpu_seconds(), rss_bytes());
    let video = metrics().video_frames.load(Ordering::Relaxed);
    let mut s = lock();
    let frames = s.frames.len();
    if let Some(p) = s.phases.last_mut()
        && p.end.is_none()
    {
        p.end = Some(t);
        p.cpu_end = cpu;
        p.frame_end = frames;
        p.video_end = video;
        p.rss_end = rss;
    }
    if !name.is_empty() {
        s.phases.push(Phase {
            name: name.to_string(),
            start: t,
            end: None,
            cpu_start: cpu,
            cpu_end: cpu,
            frame_start: frames,
            frame_end: frames,
            video_start: video,
            video_end: video,
            rss_peak: rss,
            rss_end: rss,
        });
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// Everything recorded so far, as JSON.
pub fn report() -> Value {
    phase("");
    let s = lock();
    let frames = &s.frames;
    let phases: Vec<Value> = s
        .phases
        .iter()
        .map(|p| {
            let end = p.end.unwrap_or_else(now);
            let dur = (end - p.start).max(1e-9);
            let ts = &frames[p.frame_start.min(frames.len())..p.frame_end.min(frames.len())];
            let mut gaps: Vec<f64> = ts.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
            gaps.sort_by(f64::total_cmp);
            // A "long frame" is one that took more than 1.5 refreshes at 60 Hz while frames were flowing.
            let long = gaps.iter().filter(|g| **g > 25.0).count();
            json!({
                "name": p.name,
                "seconds": dur,
                "cpu_seconds": p.cpu_end - p.cpu_start,
                "cpu_percent": (p.cpu_end - p.cpu_start) / dur * 100.0,
                "frames": ts.len(),
                "fps": ts.len() as f64 / dur,
                "frame_ms_p50": percentile(&gaps, 0.50),
                "frame_ms_p95": percentile(&gaps, 0.95),
                "frame_ms_p99": percentile(&gaps, 0.99),
                "frame_ms_max": gaps.last().copied().unwrap_or(0.0),
                "long_frames": long,
                "video_frames_rendered": p.video_end - p.video_start,
                "rss_mb_end": p.rss_end as f64 / 1048576.0,
                "rss_mb_peak": p.rss_peak as f64 / 1048576.0,
            })
        })
        .collect();
    let mut first_marks = serde_json::Map::new();
    for (name, t, _) in &s.marks {
        first_marks.entry(name.clone()).or_insert(json!(t * 1000.0));
    }
    json!({
        "app": s.app,
        "exec_to_init_ms": s.exec_to_init.map(|x| x * 1000.0),
        "marks_ms": first_marks,
        "phases": phases,
        "frames_total": frames.len(),
        // Raw data, so smoothness can be computed over motion only, outside the app.
        "frame_times_ms": frames.iter().map(|t| (t * 10000.0).round() / 10.0).collect::<Vec<_>>(),
        "commands": s.commands.iter().map(|(t, c)| json!([(t * 10000.0).round() / 10.0, c])).collect::<Vec<_>>(),
        "all_marks": s.marks.iter().map(|(n, t, _)| json!([(t * 10000.0).round() / 10.0, n])).collect::<Vec<_>>(),
        "rss_mb_peak": s.rss_peak as f64 / 1048576.0,
        "rss_mb_now": rss_bytes() as f64 / 1048576.0,
        "cpu_seconds_total": cpu_seconds(),
        "image_cache_mb": crate::images::cached_bytes() as f64 / 1048576.0,
        "samples": s.samples.iter().map(|(t, c, r)| json!([t, c, *r as f64 / 1048576.0])).collect::<Vec<_>>(),
    })
}

fn clock_ticks() -> f64 {
    // SAFETY: sysconf is always safe to call.
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if t > 0 { t as f64 } else { 100.0 }
}

/// The fields of /proc/self/stat after the command name.
fn stat_fields() -> Vec<String> {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    after.split_whitespace().map(str::to_string).collect()
}

/// User + system CPU seconds used by the whole process (all threads).
pub fn cpu_seconds() -> f64 {
    let f = stat_fields();
    // utime and stime are fields 14 and 15; after the ")" they are at 11 and 12.
    let ticks = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / clock_ticks()
}

/// Resident set size in bytes.
pub fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|x| x.parse::<u64>().ok())
        .unwrap_or(0);
    // SAFETY: sysconf is always safe to call.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    pages * page
}

/// Seconds between the kernel starting this process and now.
fn seconds_since_exec() -> Option<f64> {
    let start_ticks: f64 = stat_fields().get(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(uptime - start_ticks / clock_ticks())
}
