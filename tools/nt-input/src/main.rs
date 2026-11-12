//! Real input for the NativeTube apps running on their headless outputs.
//!
//!   nt-input <fw> click X Y [left|right|middle|back|forward]
//!   nt-input <fw> move X Y
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
//! where it was and the previously focused window gets focus back.

use std::process::Command;
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_output, wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
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
    fn event(s: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_seat" if s.seat.is_none() => s.seat = Some(reg.bind(name, version.min(7), qh, ())),
                "zwlr_virtual_pointer_manager_v1" => s.manager = Some(reg.bind(name, version.min(2), qh, ())),
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
    fn event(s: &mut Self, out: &wl_output::WlOutput, ev: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = ev
            && let Some(slot) = s.outputs.iter_mut().find(|(o, _)| o == out)
        {
            slot.1 = Some(name);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerManagerV1, _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerV1, _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
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
        self.vp.motion_absolute(self.t(), x.round() as u32, y.round() as u32, W, H);
        self.vp.frame();
        self.flush();
    }
    fn button(&self, button: u32, pressed: bool) {
        let st = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
        self.vp.button(self.t(), button, st);
        self.vp.frame();
        self.flush();
    }
    fn wheel_notch(&self, dir: i32) {
        let t = self.t();
        self.vp.axis_source(wl_pointer::AxisSource::Wheel);
        self.vp.axis_discrete(t, wl_pointer::Axis::VerticalScroll, WHEEL_STEP * dir as f64, dir);
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
        self.vp.axis_stop(self.t(), wl_pointer::Axis::VerticalScroll);
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
    hypr(&["dispatch", &format!("hl.dsp.focus({{ window = \"address:{address}\" }})")]);
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("{}", include_str!("main.rs").lines().take(10).map(|l| l.trim_start_matches("//! ")).collect::<Vec<_>>().join("\n"));
        std::process::exit(2);
    }
    let fw = args[0].to_ascii_uppercase();
    let output_name = format!("NT-{fw}");
    let class = format!("nt-{}", args[0]);
    let num = |i: usize, default: f64| args.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(default);

    // Several agents may drive their apps at once; the cursor is shared, so take turns.
    let lock_path = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("nt-input.lock");
    let _lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).ok();
    if let Some(f) = &_lock {
        let _ = f.lock();
    }

    // What to put back afterwards.
    let cursor = hypr_json(&["cursorpos"]);
    let prev_window = hypr_json(&["activewindow"]).get("address").and_then(|a| a.as_str()).map(str::to_string);
    let window = hypr_json(&["clients"])
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["class"] == class.as_str()).cloned());
    let Some(window) = window else {
        eprintln!("no window with app_id {class} (start it with tools/headless.sh run {})", args[0]);
        std::process::exit(1);
    };
    let win_addr = window["address"].as_str().unwrap_or_default().to_string();

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    let manager = state.manager.clone().expect("compositor lacks zwlr_virtual_pointer_manager_v1");
    let output = state
        .outputs
        .iter()
        .find(|(_, n)| n.as_deref() == Some(output_name.as_str()))
        .map(|(o, _)| o.clone())
        .unwrap_or_else(|| {
            eprintln!("no output {output_name}; run tools/headless.sh setup");
            std::process::exit(1)
        });
    let vp = manager.create_virtual_pointer_with_output(state.seat.as_ref(), Some(&output), &qh, ());
    let p = Pointer { conn: conn.clone(), vp, start: Instant::now() };
    queue.roundtrip(&mut state).expect("roundtrip");

    let (x, y) = (num(2, 640.0), num(3, 400.0));
    let mut needs_focus_restore = false;
    match args[1].as_str() {
        "move" => p.move_to(x, y),
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
            sleep_ms(80);
            let mut cmd = Command::new("wtype");
            if args[1] == "type" {
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

    // Focus first: focusing a window warps the cursor to it.
    if needs_focus_restore
        && let Some(prev) = prev_window
        && prev != win_addr
    {
        focus(&prev);
    }
    if let (Some(cx), Some(cy)) = (cursor["x"].as_f64(), cursor["y"].as_f64()) {
        hypr(&["dispatch", &format!("hl.dsp.cursor.move({{ x = {}, y = {} }})", cx as i64, cy as i64)]);
    }
}
