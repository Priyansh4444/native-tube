use gpui::prelude::*;
use gpui::{Context, CursorHideMode, Keystroke, Window, WindowOptions, div, rgb};

struct View;

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("cursor-hide")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .on_click(|_, window, cx| {
                eprintln!("visible before: {}", cx.is_cursor_visible());
                window.defer(cx, |window, cx| {
                    if let Ok(key) = Keystroke::parse("x") {
                        // The no-input view has no text handler, so this inserts nothing.
                        window.dispatch_keystroke(key, cx);
                        eprintln!("visible after synthetic input: {}", cx.is_cursor_visible());
                    }
                });
            })
            .child("Click to request a hidden cursor")
    }
}

fn main() {
    gpui_platform::application().run(|cx| {
        cx.set_cursor_hide_mode(CursorHideMode::OnTypingAndAction);
        let opts = WindowOptions {
            app_id: std::env::var("NT_APP_ID").ok(),
            focus: false,
            ..Default::default()
        };
        if let Err(err) = cx.open_window(opts, |_, cx| cx.new(|_| View)) {
            eprintln!("{err}");
            cx.quit();
        }
    });
}
