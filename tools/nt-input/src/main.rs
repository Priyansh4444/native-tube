//! Real input for the NativeTube apps running on their headless outputs.
//!
//!   nt-input <fw> click X Y [left|right|middle|back|forward]
//!   nt-input <fw> move X Y
//!   nt-input <fw> hover X Y OUT.png [MS]              hover, screenshot the output, restore
//!   nt-input <fw> wheel X Y TICKS [INTERVAL_MS]      mouse-wheel notches (+ scrolls down)
//!   nt-input <fw> touchpad X Y PIXELS [DURATION_MS]  two-finger swipe then lift (fling)
//!   nt-input <fw> drag X1 Y1 X2 Y2 [DURATION_MS]
//!   nt-input <fw> key KEY...                         e.g. Return, space, ctrl+l, alt+Left
//!   nt-input <fw> type TEXT
//!
//! <fw> is iced, gpui or rune; X Y are logical pixels inside that app's
//! 1280x800 output NT-<FW> (set up by tools/headless.sh). Pointer events go
//! through a wlr virtual pointer bound to that output, so they reach the app
//! exactly as a real mouse or touchpad would. Afterwards the cursor goes back
//! where it was (or onto a real screen, never a test output) and the previously focused
//! window gets focus back. A detached watchdog does the same if the process is killed.

use std::process::Command;
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_output, wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const W: u32 = 1280;
const H: u32 = 800;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;
/// libinput's (and so most compositors') value for one wheel notch.
const WHEEL_STEP: f64 = 15.0;

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<ZwlrVirtualPointerManagerV1>,
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        reg: &wl_registry::WlRegistry,
        ev: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = ev
        {
            match interface.as_str() {
                "wl_seat" if s.seat.is_none() => {
                    s.seat = Some(reg.bind(name, version.min(7), qh, ()))
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    s.manager = Some(reg.bind(name, version.min(2), qh, ()))
                }
                "wl_output" => {
                    let o = reg.bind(name, version.min(4), qh, ());
                    s.outputs.push((o, None));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        s: &mut Self,
        out: &wl_output::WlOutput,
        ev: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = ev
            && let Some(slot) = s.outputs.iter_mut().find(|(o, _)| o == out)
        {
            slot.1 = Some(name);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

struct Pointer {
    conn: Connection,
    vp: ZwlrVirtualPointerV1,
    start: Instant,
}

impl Pointer {
    fn t(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
    fn flush(&self) {
        let _ = self.conn.flush();
    }
    fn move_to(&self, x: f64, y: f64) {
        let (x, y) = (x.clamp(0.0, (W - 1) as f64), y.clamp(0.0, (H - 1) as f64));
        self.vp
            .motion_absolute(self.t(), x.round() as u32, y.round() as u32, W, H);
        self.vp.frame();
        self.flush();
    }
    fn button(&self, button: u32, pressed: bool) {
        let st = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        self.vp.button(self.t(), button, st);
        self.vp.frame();
        self.flush();
    }
    fn wheel_notch(&self, dir: i32) {
        let t = self.t();
        self.vp.axis_source(wl_pointer::AxisSource::Wheel);
        self.vp.axis_discrete(
            t,
            wl_pointer::Axis::VerticalScroll,
            WHEEL_STEP * dir as f64,
            dir,
        );
        self.vp.frame();
        self.flush();
    }
    fn finger(&self, dy: f64) {
        self.vp.axis_source(wl_pointer::AxisSource::Finger);
        self.vp.axis(self.t(), wl_pointer::Axis::VerticalScroll, dy);
        self.vp.frame();
        self.flush();
    }
    fn finger_stop(&self) {
        self.vp.axis_source(wl_pointer::AxisSource::Finger);
        self.vp
            .axis_stop(self.t(), wl_pointer::Axis::VerticalScroll);
        self.vp.frame();
        self.flush();
    }
}

fn hypr_json(args: &[&str]) -> serde_json::Value {
    Command::new("hyprctl")
        .arg("-j")
        .args(args)
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn hypr(args: &[&str]) {
    let _ = Command::new("hyprctl").args(args).output();
}

/// Hyprland 0.57's Lua dispatchers.
fn focus(address: &str) {
    hypr(&[
        "dispatch",
        &format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
    ]);
}

/// Waits (up to `max_ms`) until Hyprland reports `address` as the active window, so keys sent
/// right after a focus change aren't delivered before the client has keyboard focus.
fn wait_focus(address: &str, max_ms: u64) -> bool {
    let start = Instant::now();
    loop {
        if hypr_json(&["activewindow"])["address"].as_str() == Some(address) {
            return true;
        }
        if start.elapsed() >= Duration::from_millis(max_ms) {
            return false;
        }
        sleep_ms(15);
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Is (x, y) inside monitor `m` (Hyprland layout coordinates are logical pixels)?
fn monitor_contains(m: &serde_json::Value, x: f64, y: f64) -> bool {
    let num = |k: &str| m[k].as_f64().unwrap_or(0.0);
    let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
    let (mx, my) = (num("x"), num("y"));
    let (mw, mh) = (num("width") / scale, num("height") / scale);
    x >= mx && x < mx + mw && y >= my && y < my + mh
}

fn is_test_monitor(m: &serde_json::Value) -> bool {
    m["name"].as_str().is_some_and(|n| n.starts_with("NT-"))
}

/// Where the owner's cursor goes back to: the saved point if it is on a real screen, otherwise
/// the centre of a real screen. Never a test output, so one stranded cursor can't stay stranded.
fn safe_home(saved: &serde_json::Value) -> Option<(i64, i64)> {
    let monitors = hypr_json(&["monitors"]);
    let real: Vec<&serde_json::Value> = monitors
        .as_array()
        .map(|ms| ms.iter().filter(|m| !is_test_monitor(m)).collect())
        .unwrap_or_default();
    if let (Some(x), Some(y)) = (saved["x"].as_f64(), saved["y"].as_f64())
        && real.iter().any(|m| monitor_contains(m, x, y))
    {
        return Some((x as i64, y as i64));
    }
    let m = real
        .iter()
        .find(|m| m["focused"].as_bool() == Some(true))
        .or_else(|| real.first())?;
    let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
    let c = |pos: &str, size: &str| {
        m[pos].as_f64().unwrap_or(0.0) + m[size].as_f64().unwrap_or(0.0) / scale / 2.0
    };
    Some((c("x", "width") as i64, c("y", "height") as i64))
}

fn cursor_on_test_output() -> bool {
    let cursor = hypr_json(&["cursorpos"]);
    let (Some(x), Some(y)) = (cursor["x"].as_f64(), cursor["y"].as_f64()) else {
        return false;
    };
    hypr_json(&["monitors"]).as_array().is_some_and(|ms| {
        ms.iter()
            .any(|m| is_test_monitor(m) && monitor_contains(m, x, y))
    })
}

/// Give the owner back their screen: focus the window they had (unless it is a test window),
/// else a real monitor, then put the cursor on a real screen. Focus first: focusing a window
/// warps the cursor to it.
fn give_back(home: Option<(i64, i64)>, prev_window: Option<&str>, test_window: &str) {
    match prev_window.filter(|p| *p != test_window && !p.is_empty()) {
        Some(prev) => focus(prev),
        None => {
            if let Some(name) = hypr_json(&["monitors"]).as_array().and_then(|ms| {
                ms.iter()
                    .find(|m| !is_test_monitor(m))
                    .and_then(|m| m["name"].as_str().map(str::to_string))
            }) {
                hypr(&[
                    "dispatch",
                    &format!("hl.dsp.focus({{ monitor = \"{name}\" }})"),
                ]);
            }
        }
    }
    if let Some((x, y)) = home {
        hypr(&[
            "dispatch",
            &format!("hl.dsp.cursor.move({{ x = {x}, y = {y} }})"),
        ]);
    }
}

/// `nt-input __guard <pid> <x> <y> <prev-window|->`: a detached watchdog. When <pid> exits for
/// any reason (finished, killed, crashed) and the cursor is still on a test output, it gives the
/// owner their screen back. Started by every action, in its own process group.
fn guard(args: &[String]) {
    let pid = args.first().cloned().unwrap_or_default();
    let home = match (args.get(1), args.get(2)) {
        (Some(x), Some(y)) => x.parse().ok().zip(y.parse().ok()),
        _ => None,
    };
    let prev = args
        .get(3)
        .filter(|p| p.as_str() != "-")
        .map(String::as_str);
    let started = Instant::now();
    while std::path::Path::new(&format!("/proc/{pid}")).exists()
        && started.elapsed() < Duration::from_secs(120)
    {
        sleep_ms(50);
    }
    if cursor_on_test_output() {
        give_back(home, prev, "");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "__guard") {
        guard(&args[1..]);
        return;
    }
    // The owner's pause switch: while this file exists, nobody's test drives the real cursor.
    // `touch $XDG_RUNTIME_DIR/nt-input.paused` pauses; removing it resumes.
    // Checked in two places: the runtime dir (fast to toggle) and ~/.local/state, which survives
    // reboots. A reboot once wiped the runtime-dir file while the owner still wanted input paused.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("nt-input.paused");
    let state = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/state/nt-input.paused"));
    let paused = std::iter::once(runtime).chain(state).find(|p| p.exists());
    if let Some(paused) = paused {
        eprintln!(
            "nt-input: PAUSED by the owner ({}): they are using the machine. Don't drive the GUI \
             any other way either; report the step as needing input verification.",
            paused.display()
        );
        std::process::exit(75);
    }
    if args.first().is_some_and(|a| a == "--stay") {
        eprintln!(
            "--stay was removed: it left the owner's real cursor on a test output. \
             Use `nt-input <fw> hover X Y OUT.png [MS]`, which screenshots the hover and restores."
        );
        std::process::exit(2);
    }
    if args.len() < 2 {
        eprintln!(
            "{}",
            include_str!("main.rs")
                .lines()
                .take(10)
                .map(|l| l.trim_start_matches("//! "))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::process::exit(2);
    }
    let fw = args[0].to_ascii_uppercase();
    let output_name = format!("NT-{fw}");
    let class = format!("nttest-{}", args[0]);
    let num = |i: usize, default: f64| {
        args.get(i)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(default)
    };

    // Several agents may drive their apps at once; the cursor is shared, so take turns.
    let lock_path = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("nt-input.lock");
    let _lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok();
    if let Some(f) = &_lock {
        let _ = f.lock();
    }

    // What to put back afterwards.
    let cursor = hypr_json(&["cursorpos"]);
    // The owner's window to give focus back to; never a test window.
    let active = hypr_json(&["activewindow"]);
    let prev_window = active
        .get("address")
        .and_then(|a| a.as_str())
        .filter(|_| {
            !active["class"]
                .as_str()
                .is_some_and(|c| c.starts_with("nttest-"))
        })
        .map(str::to_string);
    let window = hypr_json(&["clients"])
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["class"] == class.as_str()).cloned());
    let Some(window) = window else {
        eprintln!(
            "no window with app_id {class} (start it with tools/headless.sh run {})",
            args[0]
        );
        std::process::exit(1);
    };
    let win_addr = window["address"].as_str().unwrap_or_default().to_string();
    let home = safe_home(&cursor);

    // A watchdog in its own process group: if this process dies mid-action (killed, timed out,
    // crashed), it still puts the owner's cursor and focus back.
    if let Ok(exe) = std::env::current_exe() {
        use std::os::unix::process::CommandExt;
        let (hx, hy) = home.map_or((String::new(), String::new()), |(x, y)| {
            (x.to_string(), y.to_string())
        });
        let _ = Command::new(exe)
            .args([
                "__guard",
                &std::process::id().to_string(),
                &hx,
                &hy,
                prev_window.as_deref().unwrap_or("-"),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn();
    }

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    let manager = state
        .manager
        .clone()
        .expect("compositor lacks zwlr_virtual_pointer_manager_v1");
    let output = state
        .outputs
        .iter()
        .find(|(_, n)| n.as_deref() == Some(output_name.as_str()))
        .map(|(o, _)| o.clone())
        .unwrap_or_else(|| {
            eprintln!("no output {output_name}; run tools/headless.sh setup");
            std::process::exit(1)
        });
    let vp =
        manager.create_virtual_pointer_with_output(state.seat.as_ref(), Some(&output), &qh, ());
    let p = Pointer {
        conn: conn.clone(),
        vp,
        start: Instant::now(),
    };
    queue.roundtrip(&mut state).expect("roundtrip");

    let (x, y) = (num(2, 640.0), num(3, 400.0));
    let mut needs_focus_restore = false;
    match args[1].as_str() {
        "move" => p.move_to(x, y),
        "hover" => {
            // Hover, screenshot the output while the pointer is still there, then restore.
            let out = args.get(4).cloned().unwrap_or_else(|| "hover.png".into());
            p.move_to(x, y);
            sleep_ms(num(5, 700.0) as u64);
            let _ = Command::new("grim")
                .args(["-o", &output_name, &out])
                .status();
        }
        "click" => {
            let button = match args.get(4).map(String::as_str) {
                Some("right") => BTN_RIGHT,
                Some("middle") => BTN_MIDDLE,
                Some("back") => BTN_SIDE,
                Some("forward") => BTN_EXTRA,
                _ => BTN_LEFT,
            };
            p.move_to(x, y);
            sleep_ms(40);
            p.button(button, true);
            sleep_ms(60);
            p.button(button, false);
            needs_focus_restore = true;
        }
        "wheel" => {
            let ticks = num(4, 1.0) as i32;
            let interval = num(5, 50.0) as u64;
            p.move_to(x, y);
            sleep_ms(40);
            for _ in 0..ticks.unsigned_abs() {
                p.wheel_notch(ticks.signum());
                sleep_ms(interval);
            }
        }
        "touchpad" => {
            let total = num(4, 600.0);
            let duration = num(5, 250.0).max(16.0);
            p.move_to(x, y);
            sleep_ms(40);
            // ~120 Hz events with an ease-out, like a real swipe, then lift.
            let steps = (duration / 8.0).ceil() as usize;
            let mut sent = 0.0;
            for i in 1..=steps {
                let f = i as f64 / steps as f64;
                let target = total * (1.0 - (1.0 - f).powi(2));
                p.finger(target - sent);
                sent = target;
                sleep_ms(8);
            }
            p.finger_stop();
        }
        "drag" => {
            let (x2, y2, duration) = (num(4, x), num(5, y), num(6, 300.0));
            p.move_to(x, y);
            sleep_ms(40);
            p.button(BTN_LEFT, true);
            let steps = (duration / 8.0).ceil().max(1.0) as usize;
            for i in 1..=steps {
                let f = i as f64 / steps as f64;
                p.move_to(x + (x2 - x) * f, y + (y2 - y) * f);
                sleep_ms(8);
            }
            p.button(BTN_LEFT, false);
            needs_focus_restore = true;
        }
        "key" | "type" => {
            focus(&win_addr);
            if !wait_focus(&win_addr, 800) {
                // Never type into whatever does have focus (it may be the owner's editor).
                eprintln!("nt-input: window {win_addr} did not take focus; no keys sent");
                give_back(home, prev_window.as_deref(), &win_addr);
                std::process::exit(3);
            }
            // wtype uploads its own keymap first. Give the client time to compile it before
            // the first key, or that key can be dropped (seen as "first key after a pause").
            sleep_ms(40);
            let mut cmd = Command::new("wtype");
            cmd.args(["-s", "60"]);
            if args[1] == "type" {
                cmd.args(["-d", "12"]);
                cmd.arg(args[2..].join(" "));
            } else {
                for k in &args[2..] {
                    let parts: Vec<&str> = k.split('+').collect();
                    let (mods, key) = parts.split_at(parts.len() - 1);
                    for m in mods {
                        cmd.args(["-M", m]);
                    }
                    cmd.args(["-k", key[0]]);
                    for m in mods.iter().rev() {
                        cmd.args(["-m", m]);
                    }
                }
            }
            let _ = cmd.status();
            sleep_ms(80);
            needs_focus_restore = true;
        }
        other => {
            eprintln!("unknown action {other}");
            std::process::exit(2);
        }
    }
    sleep_ms(30);
    p.vp.destroy();
    let _ = conn.flush();

    if needs_focus_restore || cursor_on_test_output() {
        give_back(home, prev_window.as_deref(), &win_addr);
    } else if let Some((hx, hy)) = home {
        hypr(&[
            "dispatch",
            &format!("hl.dsp.cursor.move({{ x = {hx}, y = {hy} }})"),
        ]);
    }
}
