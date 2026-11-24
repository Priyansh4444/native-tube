//! NativeTube, the iced frontend.

mod app;
mod clock;
mod ext;
mod icons;
mod imgcache;
mod interaction;
mod player_view;
mod prefs;
mod probe;
mod scroller;
mod surface;
mod theme;
mod video;
mod view;
mod widgets;

use iced::{Size, window};

/// Runs iced's tasks and subscriptions on the core's tokio runtime instead of a second pool.
struct CoreExecutor(&'static tokio::runtime::Runtime);

impl iced::Executor for CoreExecutor {
    fn new() -> Result<Self, iced::futures::io::Error> {
        nt_core::try_runtime()
            .map(Self)
            .map_err(iced::futures::io::Error::other)
    }
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.0.spawn(future);
    }
    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.0.block_on(future)
    }
    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.0.enter();
        f()
    }
}

/// iced asks wgpu for the high-performance adapter and probes every backend. On a hybrid Linux
/// laptop that wakes the discrete GPU (and the compositor then copies every frame to the
/// integrated one that scans out) and loads GL drivers we never use. A UI wants the low-power
/// adapter on Vulkan. Both knobs are wgpu's own env variables, so a user setting wins. Other
/// platforms keep wgpu's defaults (Metal on macOS).
#[cfg(target_os = "linux")]
fn prefer_integrated_gpu() {
    for (key, value) in [("WGPU_POWER_PREF", "low"), ("WGPU_BACKEND", "vulkan")] {
        if std::env::var_os(key).is_none() {
            // SAFETY: start of `main`, before iced or any thread that reads the environment.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

/// Log every panic's message and location before the default behaviour.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".into());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        eprintln!("nt-iced: panic at {location}: {message}");
        default(info);
    }));
}

/// Build iced's font database (a scan of the system fonts, used as fallback for scripts Roboto
/// lacks) and load the bundled Roboto faces on a thread of their own, while the window and the
/// GPU device are being created. Without this, the scan runs inside the first layout, on the
/// UI thread, before the first frame. iced's own `.font()` loads later are no-ops (it skips
/// bytes it has already loaded).
fn prewarm_fonts() {
    let _ = std::thread::Builder::new()
        .name("nt-fonts".into())
        .spawn(|| {
            let fs = iced::advanced::graphics::text::font_system();
            if let Ok(mut fs) = fs.write() {
                for bytes in [
                    theme::ROBOTO_REGULAR,
                    theme::ROBOTO_MEDIUM,
                    theme::ROBOTO_BOLD,
                ] {
                    fs.load_font(std::borrow::Cow::Borrowed(bytes));
                }
            }
            scroller::trace_event("fonts_ready");
        });
}

fn main() -> iced::Result {
    // SAFETY: the process is still single-threaded and the toolkit and EGL are uninitialized.
    unsafe { nt_core::gpu::select_egl_vendor() };

    nt_core::metrics::init("iced");
    install_panic_hook();
    #[cfg(target_os = "linux")]
    prefer_integrated_gpu();
    nt_core::autopilot::start();
    clock::init();
    scroller::trace_event("main");
    prewarm_fonts();

    let window = window::Settings {
        size: Size::new(1280.0, 800.0),
        min_size: Some(Size::new(480.0, 360.0)),
        resizable: true,
        transparent: surface::enabled(),
        exit_on_close_request: false,
        #[cfg(target_os = "linux")]
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
        .style(|_, _| iced::theme::Style {
            background_color: if surface::enabled() {
                iced::Color::TRANSPARENT
            } else {
                theme::BG
            },
            text_color: theme::TEXT,
        })
        .font(theme::ROBOTO_REGULAR)
        .font(theme::ROBOTO_MEDIUM)
        .font(theme::ROBOTO_BOLD)
        .default_font(theme::REGULAR)
        .executor::<CoreExecutor>()
        .window(window)
        .antialiasing(false)
        .run()
}
