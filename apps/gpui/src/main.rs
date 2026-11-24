//! NativeTube, the GPUI frontend.

mod app;
mod chrome;
mod content;
mod hardware;
mod icons;
mod images;
mod inhibit;
mod pages;
mod player;
mod playlist;
mod prefs;
mod preview;
mod probe;
mod rail;
mod theme;
mod top_bar;

use app::{CloseWindow, Quit};
use gpui::{
    AppContext as _, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::{Root, Theme, ThemeMode};
use std::borrow::Cow;

/// The shared Roboto files, embedded: text never depends on a system font.
const ROBOTO: [&[u8]; 3] = [
    include_bytes!("../../../assets/fonts/Roboto-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Roboto-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Roboto-Bold.ttf"),
];

fn apply_theme(cx: &mut gpui::App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let t = Theme::global_mut(cx);
    t.font_family = "Roboto".into();
    let col = &mut t.colors;
    // NtApp paints the window background, including around the optional native video.
    col.background = gpui::transparent_black();
    col.foreground = theme::text();
    col.muted_foreground = theme::text2();
    col.border = theme::search_border();
    col.input = theme::search_border();
    col.popover = theme::surface();
    col.popover_foreground = theme::text();
    col.caret = theme::text();
    col.ring = theme::search_focus();
    col.selection = theme::c(0x264f78);
    t.tokens.background = gpui::transparent_black().into();
}

/// Log panics with their location before the default hook runs.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-string panic payload)".to_string());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        eprintln!("nt-gpui: PANIC at {at}: {msg}");
        default(info);
    }));
}

fn main() {
    // SAFETY: the process is still single-threaded and the toolkit and EGL are uninitialized.
    unsafe { nt_core::gpu::select_egl_vendor() };

    nt_core::metrics::init("gpui");
    nt_core::autopilot::start();
    install_panic_hook();

    gpui_platform::application()
        .with_assets(icons::AppAssets)
        .run(|cx| {
            if let Err(e) = cx
                .text_system()
                .add_fonts(ROBOTO.iter().map(|b| Cow::Borrowed(*b)).collect())
            {
                eprintln!("nt-gpui: can't load the bundled Roboto: {e}");
            }
            gpui_component::init(cx);
            apply_theme(cx);
            cx.bind_keys([
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-w", CloseWindow, None),
            ]);
            // Quit even when no view is focused (the root view also handles it, saving state).
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &CloseWindow, cx| cx.quit());
            // macOS application menu; ignored where there is no native menu bar.
            cx.set_menus([
                Menu::new("NativeTube").items([MenuItem::action("Quit NativeTube", Quit)])
            ]);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            let app_id = std::env::var("NT_APP_ID").unwrap_or_else(|_| "nt-gpui".to_string());
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(1280.0), px(800.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("NativeTube".into()),
                    ..Default::default()
                }),
                app_id: Some(app_id),
                window_min_size: Some(size(px(480.0), px(360.0))),
                focus: false,
                // Never throttle an unfocused window's frames; an idle app draws nothing anyway.
                inactive_frame_interval: None,
                low_power_frame_interval: None,
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| app::NtApp::new(window, cx));
                let app = view.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    let _ = app.update(cx, |a, cx| a.close_playback(cx));
                    true
                });
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("nt-gpui: can't open a window: {e}");
                cx.quit();
            }
        });
}
