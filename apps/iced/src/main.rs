//! NativeTube, the iced frontend.

mod app;
mod clock;
mod ext;
mod imgcache;
mod scroller;
mod theme;
mod video;
mod view;
mod widgets;

use iced::{Size, window};

/// Runs iced's tasks and subscriptions on the core's tokio runtime instead of a second pool.
struct CoreExecutor;

impl iced::Executor for CoreExecutor {
    fn new() -> Result<Self, iced::futures::io::Error> {
        Ok(CoreExecutor)
    }
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        nt_core::runtime().spawn(future);
    }
    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        nt_core::runtime().block_on(future)
    }
    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = nt_core::runtime().enter();
        f()
    }
}

/// Core workaround (see NOTES.md, "Core requests"): `bench/scenario.json` writes single commands
/// as `{ "do": { "cmd": ... } }`, but `autopilot::Step::Do(Command)` is an untagged newtype that
/// only accepts the bare command object, so the whole scenario is rejected. Unwrap those steps
/// into a copy and point `NT_AUTOPILOT` at it. Does nothing when the file already parses as is.
fn normalize_scenario() {
    let Some(path) = std::env::var_os("NT_AUTOPILOT") else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    if serde_json::from_slice::<Vec<nt_core::autopilot::Step>>(&bytes).is_ok() {
        return;
    }
    let Ok(serde_json::Value::Array(steps)) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let steps: Vec<serde_json::Value> = steps
        .into_iter()
        .map(|step| match step {
            serde_json::Value::Object(mut m) if m.len() == 1 && m.contains_key("do") => m.remove("do").unwrap_or_default(),
            other => other,
        })
        .collect();
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let out = dir.join(format!("nt-iced-scenario-{}.json", std::process::id()));
    if serde_json::to_vec(&steps).ok().and_then(|b| std::fs::write(&out, b).ok()).is_some() {
        // SAFETY: called at the very start of `main`; the only other thread (the metrics
        // sampler) never reads the environment.
        unsafe { std::env::set_var("NT_AUTOPILOT", &out) };
    }
}

/// iced asks wgpu for the high-performance adapter and probes every backend. On a hybrid
/// laptop that wakes the discrete GPU (and the compositor then has to copy every frame to the
/// integrated one that scans out), and loads GL drivers we never use. A UI wants the low-power
/// adapter on Vulkan; both knobs are wgpu's own env variables, so a user setting wins.
fn prefer_integrated_gpu() {
    for (key, value) in [("WGPU_POWER_PREF", "low"), ("WGPU_BACKEND", "vulkan")] {
        if std::env::var_os(key).is_none() {
            // SAFETY: start of `main`, before iced or any thread that reads the environment.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

fn main() -> iced::Result {
    nt_core::metrics::init("iced");
    normalize_scenario();
    prefer_integrated_gpu();
    nt_core::autopilot::start();
    clock::init();

    let window = window::Settings {
        size: Size::new(1280.0, 800.0),
        min_size: Some(Size::new(640.0, 480.0)),
        resizable: true,
        platform_specific: window::settings::PlatformSpecific {
            application_id: std::env::var("NT_APP_ID").unwrap_or_else(|_| "nt-iced".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    iced::application(app::App::boot, app::App::update, app::App::view)
        .title("NativeTube")
        .subscription(app::App::subscription)
        .theme(|_: &app::App| theme::theme())
        .style(|_, _| iced::theme::Style { background_color: theme::BG, text_color: theme::TEXT })
        .executor::<CoreExecutor>()
        .window(window)
        .antialiasing(false)
        .run()
}
