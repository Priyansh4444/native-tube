//! NativeTube on Rune.

mod app;
mod images;
mod pages;
mod preview;
mod probe;
mod scroller;
mod theme;
mod video;
mod view;
mod watch;
#[cfg(target_os = "linux")]
mod wayland_video;

use rune::{AppSpec, FrameOutcome, KeyEvent, Tx};

fn poll(s: &mut app::App, _clock: f64, tx: &mut Tx) -> bool {
    s.poll(tx)
}

fn on_key(s: &mut app::App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.on_key(ev, tx)
}

/// Logs a panic's message and location to stderr, then runs the default hook.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".into());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        eprintln!("nt-rune: panic at {at}: {msg}");
        default(info);
    }));
}

fn main() {
    // SAFETY: the process is still single-threaded and the toolkit and EGL are uninitialized.
    unsafe { nt_core::gpu::select_egl_vendor() };

    probe::init();
    nt_core::metrics::init("rune");
    nt_core::autopilot::start();
    install_panic_hook();
    // Test runs (tools/headless.sh) name the window through NT_APP_ID.
    rune::set_app_id(Box::leak(
        std::env::var("NT_APP_ID")
            .unwrap_or_else(|_| "nt-rune".to_string())
            .into_boxed_str(),
    ));
    let spec = AppSpec {
        min_size: Some((480.0, 360.0)),
        poll: Some(poll),
        on_key: Some(on_key),
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        ..AppSpec::new("NativeTube", (1280.0, 800.0), app::init, view::view)
    };
    // macOS: winit's application menu (About, Hide, Quit ⌘Q) stays; standard_menus adds Edit
    // and Window (Close ⌘W). Closing the window quits on every platform (rune::App's default).
    let app = rune::App::new(spec)
        .standard_menus()
        .on_close(video::checkpoint_on_exit)
        .on_quit(video::checkpoint_on_exit);
    #[cfg(target_os = "linux")]
    let app = if wayland_video::enabled() {
        app.window_options(rune::WindowOptions {
            transparent: true,
            ..Default::default()
        })
        .on_window(wayland_video::window_created)
    } else {
        app
    };
    app::set_handle(app.handle());
    if let Some(h) = app::handle() {
        let h = h.clone();
        nt_core::autopilot::set_waker(move || h.update_if(|s, tx| s.drain_autopilot(tx)));
    }
    app.on_frame_encoded(|f| {
        // Encoded, submitted and presented: the closest hook to presentation Rune offers.
        if f.outcome() == FrameOutcome::Drawn {
            nt_core::metrics::frame();
            app::flush_marks();
        }
    })
    .on_frame_drawn(probe::drawn)
    .run();
}
