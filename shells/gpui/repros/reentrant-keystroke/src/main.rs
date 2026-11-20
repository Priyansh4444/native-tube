use gpui::prelude::*;
use gpui::{Context, Keystroke, Window, WindowOptions, div, rgb};

struct View;

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .child("Synthetic input dispatched without re-entering the entity")
    }
}

fn inject(window: &mut Window, cx: &mut gpui::App) {
    if let Ok(stroke) = Keystroke::parse("f10") {
        window.dispatch_keystroke(stroke, cx);
    }
}

fn main() {
    gpui_platform::application().run(|cx| {
        let opts = WindowOptions {
            app_id: std::env::var("NT_APP_ID").ok(),
            focus: false,
            ..Default::default()
        };
        if let Err(err) = cx.open_window(opts, |window, cx| {
            cx.new(|cx| {
                if std::env::var_os("REPRO_WINDOW_DEFER").is_some() {
                    window.defer(cx, |window, cx| {
                        window.refresh();
                        inject(window, cx);
                    });
                } else {
                    cx.defer_in(window, |_, window, cx| {
                        window.refresh();
                        inject(window, cx);
                    });
                }
                View
            })
        }) {
            eprintln!("{err}");
            cx.quit();
        }
    });
}
