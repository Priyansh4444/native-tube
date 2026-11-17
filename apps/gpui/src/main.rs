//! NativeTube, the GPUI frontend.

mod app;
mod images;
mod pages;
mod theme;
mod video;
mod views;

use gpui::{
    AppContext as _, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::{Root, Theme, ThemeMode};
use std::borrow::Cow;

gpui_kit_assets::icon_assets!(
    ExtraIcons,
    [
        House,
        Users,
        RotateCcwClock,
        Clock,
        Volume2,
        VolumeX,
        Bookmark,
        BookmarkCheck,
        Trash,
        ThumbsUp
    ]
);

/// The kit's default icon bundle plus the few extra Lucide icons this app uses.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut v = ExtraIcons.list(path)?;
        v.extend(gpui_kit_assets::Assets.list(path)?);
        Ok(v)
    }
}

fn apply_theme(cx: &mut gpui::App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let t = Theme::global_mut(cx);
    let col = &mut t.colors;
    col.background = theme::bg();
    col.foreground = theme::text();
    col.muted_foreground = theme::text2();
    col.border = theme::divider();
    col.input = theme::divider();
    col.popover = theme::c(0x212121);
    col.popover_foreground = theme::text();
    col.skeleton = theme::raised();
    col.slider_bar = theme::text();
    col.slider_thumb = theme::text();
    col.caret = theme::text();
    col.ring = theme::c(0x3ea6ff);
    col.button = theme::raised();
    col.button_hover = theme::hover();
    col.button_active = theme::hover();
    col.button_foreground = theme::text();
    col.secondary = theme::raised();
    col.secondary_hover = theme::hover();
    col.secondary_active = theme::hover();
    col.secondary_foreground = theme::text();
}

/// Core request: `autopilot::Step::Do(Command)` is an untagged newtype, so it only accepts the
/// command object itself, while `bench/scenario.json` writes single commands as
/// `{ "do": { "cmd": ... } }` and so fails to parse as a whole. If (and only if) the scenario
/// doesn't parse, write a copy with each `{ "do": X }` step replaced by `X` and point
/// `NT_AUTOPILOT` at it. The steps and their timing are otherwise unchanged.
fn fix_scenario() {
    use nt_core::autopilot::Step;
    let Some(path) = std::env::var_os("NT_AUTOPILOT") else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    if serde_json::from_slice::<Vec<Step>>(&bytes).is_ok() {
        return;
    }
    let Ok(serde_json::Value::Array(steps)) = serde_json::from_slice::<serde_json::Value>(&bytes)
    else {
        return;
    };
    let steps: Vec<serde_json::Value> = steps
        .into_iter()
        .map(|step| match step.as_object() {
            Some(o) if o.len() == 1 && o.contains_key("do") => o["do"].clone(),
            _ => step,
        })
        .collect();
    let fixed = serde_json::Value::Array(steps);
    if serde_json::from_value::<Vec<Step>>(fixed.clone()).is_err() {
        return;
    }
    let out = std::env::temp_dir().join(format!("nt-gpui-scenario-{}.json", std::process::id()));
    if std::fs::write(&out, fixed.to_string()).is_ok() {
        // SAFETY: called on the main thread before the autopilot or any UI thread exists; the
        // metrics sampler thread never touches the environment.
        unsafe { std::env::set_var("NT_AUTOPILOT", &out) };
        eprintln!(
            "nt-gpui: scenario rewritten to {} (see Core requests in NOTES.md)",
            out.display()
        );
    }
}

fn main() {
    nt_core::metrics::init("gpui");
    fix_scenario();
    nt_core::autopilot::start();

    gpui_platform::application()
        .with_assets(AppAssets)
        .run(|cx| {
            gpui_component::init(cx);
            apply_theme(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(1280.0), px(800.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("NativeTube".into()),
                    ..Default::default()
                }),
                app_id: Some(std::env::var("NT_APP_ID").unwrap_or_else(|_| "nt-gpui".to_string())),
                window_min_size: Some(size(px(640.0), px(480.0))),
                focus: false,
                // The window is never focused on its headless output, and an idle app draws
                // nothing anyway; don't let GPUI halve the frame rate of an unfocused window.
                inactive_frame_interval: None,
                low_power_frame_interval: None,
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| app::NtApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("nt-gpui: can't open a window: {e}");
                cx.quit();
            }
        });
}
