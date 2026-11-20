use crate::model::{Action, Event, Property, State, TargetId, Trace, Violation};
use crate::properties::{Properties, health, violation};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const STARTUP_MS: u64 = 10_000;
const STARTUP_LAYOUT_MS: u64 = 300;
const HEARTBEAT_MS: u64 = 3_000;
const HEALTH_MS: u64 = 175;
const SCROLL_SETTLE_MS: u64 = 200;
// Keep time for a final action deadline and app cleanup within the two-minute slice.
const ACTION_CUTOFF_MS: u64 = 108_000;
const SESSION_END_MS: u64 = 118_000;
const POLL: Duration = Duration::from_millis(20);
const KEYS: &[&str] = &[
    "Escape",
    "Return",
    "alt+Left",
    "alt+Right",
    "k",
    "j",
    "l",
    "f",
    "t",
    "space",
    "slash",
];
const QUERIES: &[&str] = &["rust", "rust programming", "rust prog"];

pub struct Runner {
    pub root: PathBuf,
    pub out: PathBuf,
    script: PathBuf,
    input: PathBuf,
    runtime: PathBuf,
}

pub struct Run {
    pub trace: Trace,
    pub missing_probe: bool,
}

impl Runner {
    pub fn new(root: PathBuf, out: PathBuf) -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join("native-tube-headless");
        Self {
            script: root.join("tools/headless.sh"),
            input: std::env::var_os("NT_INPUT").map_or_else(
                || root.join("tools/nt-input/target/release/nt-input"),
                PathBuf::from,
            ),
            root,
            out,
            runtime,
        }
    }

    pub fn run(
        &self,
        app: &Path,
        seed: u64,
        steps: usize,
        minutes: Option<f64>,
        replay: Option<&[Action]>,
        name: &str,
    ) -> io::Result<Run> {
        let dir = self.out.join(name);
        fs::create_dir(&dir)?;
        let data = tempfile::tempdir_in(&dir)?;
        let mut app = Session::launch(self, app, data.path())?;
        let mut properties = Properties::default();
        let mut trace = Trace {
            version: 1,
            app: app.binary.display().to_string(),
            seed,
            events: Vec::new(),
            violation: None,
        };
        let mut rng = StdRng::seed_from_u64(seed);
        let mut index = 0;
        let limit = minutes.map_or(ACTION_CUTOFF_MS, |minutes| {
            ((minutes * 60_000.0) as u64).min(ACTION_CUTOFF_MS)
        });
        let mut missing_probe = false;
        let result = (|| {
            loop {
                app.sample(&mut properties, &mut trace)?;
                let elapsed = app.elapsed_ms();
                if elapsed >= SESSION_END_MS {
                    return Err(violation(Property::Driver, "two-minute GUI slice elapsed"));
                }
                if let Some(state) = properties.latest() {
                    if index == 0
                        && elapsed < STARTUP_MS
                        && (state.loading
                            || !app.startup_layout_ready(elapsed)
                            || app.geometry().is_err())
                    {
                        std::thread::sleep(POLL);
                        continue;
                    }
                    if properties.settled() {
                        if index >= steps {
                            return Ok(());
                        }
                        if elapsed >= limit {
                            return if replay.is_some() {
                                Err(violation(
                                    Property::Driver,
                                    "replay exceeded the two-minute GUI slice",
                                ))
                            } else {
                                Ok(())
                            };
                        }
                        if !app.scroll_is_still(elapsed) {
                            std::thread::sleep(POLL);
                            continue;
                        }
                        let action = if let Some(actions) = replay {
                            actions[index].clone()
                        } else {
                            generate(&mut rng, state)
                        };
                        let commands = app.commands(state, &action)?;
                        // Search typing is preparation. Start its deadline at Return.
                        for command in &commands[..commands.len() - 1] {
                            app.input(command, &mut properties, &mut trace)?;
                        }
                        let elapsed_ms = app.elapsed_ms();
                        trace.events.push(Event::Action {
                            elapsed_ms,
                            action: action.clone(),
                        });
                        properties.action(elapsed_ms, &action)?;
                        app.input(&commands[commands.len() - 1], &mut properties, &mut trace)?;
                        index += 1;
                    }
                } else if elapsed >= STARTUP_MS {
                    missing_probe = true;
                    return Err(violation(Property::Probe, "no NT_STATE within 10 seconds"));
                }
                std::thread::sleep(POLL);
            }
        })();
        if let Err(mut failure) = result {
            let screenshot = dir.join("violation.png");
            let result = Command::new(&self.script)
                .args(["shot", "bench"])
                .arg(&screenshot)
                .output();
            if result.is_ok_and(|output| output.status.success()) {
                failure.screenshot = Some(screenshot.display().to_string());
            }
            trace.violation = Some(failure);
        }
        fs::copy(self.runtime.join("bench.log"), dir.join("app.log"))?;
        drop(app);
        let file = File::create(dir.join("trace.json"))?;
        serde_json::to_writer_pretty(file, &trace)?;
        Ok(Run {
            trace,
            missing_probe,
        })
    }
}

struct Session<'a> {
    runner: &'a Runner,
    binary: PathBuf,
    pid: u32,
    supervisor: u32,
    lease: Option<UnixStream>,
    started: Instant,
    reader: Option<BufReader<File>>,
    pending_log: Vec<u8>,
    first_probe: Option<u64>,
    last_probe: Option<u64>,
    last_scroll_change: Option<u64>,
    last_health: Option<u64>,
    _lock: File,
}

impl<'a> Session<'a> {
    fn launch(runner: &'a Runner, binary: &Path, data: &Path) -> io::Result<Self> {
        let binary = binary.canonicalize()?;
        let _ = fs::metadata(&runner.input)?;
        fs::create_dir_all(&runner.runtime)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(runner.runtime.join("bench.explore.lock"))?;
        lock.try_lock().map_err(|error| {
            io::Error::new(io::ErrorKind::ResourceBusy, format!("NT-BENCH: {error}"))
        })?;
        let app_pid = fs::read_to_string(runner.runtime.join("bench.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        if app_pid.is_some_and(process_alive) || clients()?.iter().any(is_test_client) {
            // One GUI-driving job at a time on the whole desktop, whoever started it.
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "another native test window or NT-BENCH session is active",
            ));
        }
        let guard_dir = tempfile::tempdir()?;
        let socket = guard_dir.path().join("guard");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let started = Instant::now();
        let output = Command::new(&runner.script)
            .args(["run", "bench"])
            .arg(std::env::current_exe()?)
            .arg("--guarded-app")
            .arg(socket)
            .arg(runner.runtime.join("bench.pid"))
            .arg(&binary)
            .env("NT_PROBE", "1")
            .env("NT_LIVE", "0")
            .env("NT_FIXTURE_MODE", "replay")
            .env("NT_MUTE", "1")
            .env("NT_NO_MPRIS", "1")
            .env("NT_DATA_DIR", data)
            .env_remove("NT_AUTOPILOT")
            .env_remove("NT_REPORT")
            .current_dir(&runner.root)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "headless launch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let supervisor = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u32>()
            .map_err(io::Error::other)?;
        let (lease, pid) = accept_app(&listener, supervisor)?;
        let mut session = Self {
            runner,
            binary,
            pid,
            supervisor,
            lease: Some(lease),
            started,
            reader: None,
            pending_log: Vec::new(),
            first_probe: None,
            last_probe: None,
            last_scroll_change: None,
            last_health: None,
            _lock: lock,
        };
        session.reader = Some(BufReader::new(File::open(
            runner.runtime.join("bench.log"),
        )?));
        Ok(session)
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn startup_layout_ready(&self, elapsed_ms: u64) -> bool {
        // The first rendered probe may still describe the toolkit's initial
        // surface size, before Hyprland configures the tiled window.
        self.first_probe
            .is_some_and(|first| elapsed_ms - first >= STARTUP_LAYOUT_MS)
    }

    fn scroll_is_still(&self, elapsed_ms: u64) -> bool {
        self.last_scroll_change
            .is_none_or(|previous| elapsed_ms - previous >= SCROLL_SETTLE_MS)
    }

    fn sample(&mut self, properties: &mut Properties, trace: &mut Trace) -> Result<(), Violation> {
        let elapsed_ms = self.elapsed_ms();
        let reader = self.reader.as_mut().unwrap();
        loop {
            let count = reader
                .read_until(b'\n', &mut self.pending_log)
                .map_err(|error| violation(Property::Probe, error.to_string()))?;
            if count == 0 || self.pending_log.last() != Some(&b'\n') {
                break;
            }
            let line = String::from_utf8_lossy(&self.pending_log);
            if let Some(json) = line.trim_end().strip_prefix("NT_STATE ") {
                let state: State = serde_json::from_str(json).map_err(|error| {
                    violation(
                        Property::Probe,
                        format!("invalid NT_STATE: {error}; {json}"),
                    )
                })?;
                // Position-dependent clicks must address the reported target. A wheel or
                // fling can move it by a full row between a probe and delivery.
                if properties.latest().is_some_and(|previous| {
                    previous.page == state.page && (previous.scroll_y - state.scroll_y).abs() > 0.25
                }) {
                    self.last_scroll_change = Some(elapsed_ms);
                }
                trace.events.push(Event::State {
                    elapsed_ms,
                    state: Box::new(state.clone()),
                });
                self.first_probe.get_or_insert(elapsed_ms);
                self.last_probe = Some(elapsed_ms);
                properties.observe(elapsed_ms, state)?;
            } else if line.trim_start().starts_with("PANIC") || line.contains(" panicked at ") {
                return Err(violation(Property::Panic, line.trim_end()));
            }
            self.pending_log.clear();
        }

        health(process_alive(self.pid), None)?;
        if self
            .last_health
            .is_none_or(|previous| elapsed_ms - previous >= HEALTH_MS)
        {
            self.last_health = Some(elapsed_ms);
            let pss_kib =
                pss(self.pid).map_err(|error| violation(Property::Pss, error.to_string()))?;
            trace.events.push(Event::Health {
                elapsed_ms,
                pss_kib,
            });
            health(true, Some(pss_kib))?;
        }
        properties.check_deadlines(elapsed_ms)?;
        if self
            .last_probe
            .is_some_and(|previous| elapsed_ms - previous > HEARTBEAT_MS)
        {
            return Err(violation(
                Property::Probe,
                "NT_STATE heartbeat stopped for over 3 seconds",
            ));
        }
        Ok(())
    }

    fn geometry(&self) -> Result<Geometry, Violation> {
        // Recheck before any real input, including keys, in case the window moved or closed.
        Geometry::read(self.pid).map_err(|error| violation(Property::Driver, error.to_string()))
    }

    fn commands(&mut self, state: &State, action: &Action) -> Result<Vec<Vec<String>>, Violation> {
        let geometry = self.geometry()?;
        let hit = |target: &TargetId| {
            let target = target.find(state).ok_or_else(|| {
                violation(Property::Driver, format!("target unavailable: {target:?}"))
            })?;
            let [x, y, w, h] = target.rect;
            if [x, y, w, h].iter().any(|value| !value.is_finite()) || w <= 0.0 || h <= 0.0 {
                return Err(violation(
                    Property::Probe,
                    format!("invalid target rectangle: {target:?}"),
                ));
            }
            let x = (x + w / 2.0).round();
            let y = (y + h / 2.0).round();
            if !(0.0..f64::from(geometry.width)).contains(&x)
                || !(0.0..f64::from(geometry.height)).contains(&y)
            {
                return Err(violation(
                    Property::Probe,
                    format!("target center outside app: {target:?}"),
                ));
            }
            Ok(geometry.point([x as i32, y as i32]))
        };
        let pointer = |kind: &str, [x, y]: [i32; 2], amount: String| {
            vec![kind.into(), x.to_string(), y.to_string(), amount]
        };
        let key = |key: &str| vec!["key".into(), key.into()];
        Ok(match action {
            Action::Click { target, right } => vec![pointer(
                "click",
                hit(target)?,
                (if *right { "right" } else { "left" }).into(),
            )],
            Action::Wheel { point, ticks } => {
                vec![pointer("wheel", geometry.point(*point), ticks.to_string())]
            }
            Action::Fling { point, pixels } => vec![pointer(
                "touchpad",
                geometry.point(*point),
                pixels.to_string(),
            )],
            Action::Search { target, text } => vec![
                pointer("click", hit(target)?, "left".into()),
                key("ctrl+a"),
                vec!["type".into(), text.clone()],
                key("Return"),
            ],
            Action::Key { key: name } => vec![key(name)],
        })
    }

    fn input(
        &mut self,
        args: &[String],
        properties: &mut Properties,
        trace: &mut Trace,
    ) -> Result<(), Violation> {
        self.geometry()?;
        let mut command = Command::new(&self.runner.input);
        command.arg("bench").args(args).stdin(Stdio::null());
        // nt-input's own watchdog restores the desktop if this child is interrupted.
        crate::guard::bind_child(&mut command, libc::SIGTERM);
        let mut child = command
            .spawn()
            .map_err(|error| violation(Property::Driver, error.to_string()))?;
        let start = Instant::now();
        let mut failure = None;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return failure.map_or(Ok(()), Err),
                Ok(Some(status)) => {
                    return Err(violation(
                        Property::Driver,
                        format!("nt-input {args:?}: {status}"),
                    ));
                }
                Ok(None) => {}
                Err(error) => return Err(violation(Property::Driver, error.to_string())),
            }
            // Complete an in-progress input before returning a violation, so nt-input can
            // restore focus and pointer. The caller samples again after the child exits.
            if start.elapsed() > Duration::from_secs(5) || self.elapsed_ms() >= SESSION_END_MS {
                // Stop it gently, then for sure. Either way nt-input's detached watchdog
                // gives the cursor and focus back once it has exited.
                // SAFETY: our own unreaped child, so the PID cannot have been reused.
                unsafe { libc::kill(child.id().cast_signed(), libc::SIGTERM) };
                let stopping = Instant::now();
                while stopping.elapsed() < Duration::from_secs(1)
                    && child.try_wait().is_ok_and(|status| status.is_none())
                {
                    std::thread::sleep(POLL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(violation(
                    Property::Driver,
                    format!("nt-input {args:?} did not finish"),
                ));
            }
            if failure.is_none() {
                failure = self.sample(properties, trace).err();
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        drop(self.lease.take());
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(1) && process_alive(self.supervisor) {
            std::thread::sleep(POLL);
        }
        // The kernel also stops the app if the guardian itself is killed.
        if process_alive(self.supervisor) {
            // SAFETY: the guardian still owns the PID read from our headless launch.
            unsafe { libc::kill(self.supervisor.cast_signed(), libc::SIGKILL) };
        }
        let marker = self.runner.runtime.join("bench.pid");
        if fs::read_to_string(&marker).is_ok_and(|pid| pid.trim() == self.supervisor.to_string()) {
            let _ = fs::remove_file(marker);
        }

        // Hyprland can retain a just-closed client for its exit animation.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(1)
            && clients().is_ok_and(|windows| {
                windows
                    .iter()
                    .any(|window| is_bench_client(window) && window["pid"] == self.pid)
            })
        {
            std::thread::sleep(POLL);
        }
    }
}

fn accept_app(listener: &UnixListener, supervisor: u32) -> io::Result<(UnixStream, u32)> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) && process_alive(supervisor) {
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut reply = String::new();
                BufReader::new(&socket).read_line(&mut reply)?;
                let pid = reply
                    .split_whitespace()
                    .next()
                    .and_then(|pid| pid.parse().ok())
                    .ok_or_else(|| {
                        io::Error::other("headless guardian did not report an app PID")
                    })?;
                return Ok((socket, pid));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("headless guardian never connected"))
}

fn process_alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, after)| after.starts_with(['Z', 'X']))
        })
        .is_some_and(|dead| !dead)
}

fn pss(pid: u32) -> io::Result<u64> {
    fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))?
        .lines()
        .find_map(|line| {
            line.strip_prefix("Pss:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .ok_or_else(|| io::Error::other(format!("missing Pss in /proc/{pid}/smaps_rollup")))
}

fn hypr_json(args: &[&str]) -> io::Result<Value> {
    let output = Command::new("hyprctl").arg("-j").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "hyprctl {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn clients() -> io::Result<Vec<Value>> {
    Ok(serde_json::from_value(hypr_json(&["clients"])?)?)
}

fn is_bench_client(window: &Value) -> bool {
    window["class"] == "nttest-bench"
}

fn is_test_client(window: &Value) -> bool {
    window["class"]
        .as_str()
        .is_some_and(|class| class.starts_with("nttest-"))
}

#[derive(Clone, Copy)]
struct Geometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Geometry {
    fn read(pid: u32) -> io::Result<Self> {
        let monitors: Vec<Value> = serde_json::from_value(hypr_json(&["monitors", "all"])?)?;
        let monitor = monitors
            .iter()
            .find(|monitor| monitor["name"] == "NT-BENCH" && monitor["scale"] == 1.0);
        let missing = || io::Error::other("app window is not mapped on the 1x NT-BENCH output");
        let monitor = monitor.ok_or_else(missing)?;
        let clients = clients()?;
        if clients
            .iter()
            .any(|window| is_test_client(window) && !is_bench_client(window))
        {
            return Err(io::Error::other("another native test window became active"));
        }
        let mut matching = clients.iter().filter(|window| is_bench_client(window));
        let window = matching.next().ok_or_else(missing)?;
        if matching.next().is_some() || window["pid"] != pid || window["monitor"] != monitor["id"] {
            return Err(missing());
        }
        let number = |value: &Value| {
            value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(missing)
        };
        Ok(Self {
            x: number(&window["at"][0])? - number(&monitor["x"])?,
            y: number(&window["at"][1])? - number(&monitor["y"])?,
            width: number(&window["size"][0])?,
            height: number(&window["size"][1])?,
        })
    }

    fn point(self, [x, y]: [i32; 2]) -> [i32; 2] {
        [
            self.x.saturating_add(x).clamp(0, 1279),
            self.y.saturating_add(y).clamp(0, 799),
        ]
    }
}

fn generate(rng: &mut StdRng, state: &State) -> Action {
    let mut choices = Vec::new();
    let mut add = |weight, action| choices.push((weight, action));
    for (index, target) in state.targets.iter().enumerate() {
        let id = TargetId::from_visible(&state.targets, index);
        if target.kind == "search_field" {
            add(
                20,
                Action::Search {
                    target: id,
                    text: QUERIES[rng.random_range(0..QUERIES.len())].into(),
                },
            );
            continue;
        }
        let video = matches!(target.kind.as_str(), "card" | "row" | "related");
        add(
            if video { 3 } else { 2 },
            Action::Click {
                target: id.clone(),
                right: false,
            },
        );
        if video {
            add(
                1,
                Action::Click {
                    target: id,
                    right: true,
                },
            );
        }
    }
    let point = [940, 580];
    add(
        12,
        Action::Wheel {
            point,
            ticks: if rng.random_bool(0.67) { 3 } else { -3 },
        },
    );
    add(
        10,
        Action::Fling {
            point,
            pixels: if rng.random_bool(0.67) { 630 } else { -630 },
        },
    );
    for &key in KEYS {
        let weight = match key {
            "Escape" if state.menu.is_some() => 16,
            "alt+Left" if state.back_depth > 0 => 12,
            "alt+Right" if state.forward_depth > 0 => 8,
            _ => 1,
        };
        add(weight, Action::Key { key: key.into() });
    }
    let mut pick = rng.random_range(0..choices.iter().map(|(weight, _)| weight).sum::<i32>());
    for (weight, action) in choices {
        if pick < weight {
            return action;
        }
        pick -= weight;
    }
    unreachable!("weights and draw have the same range")
}
