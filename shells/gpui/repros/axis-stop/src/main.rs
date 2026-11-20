use gpui::prelude::*;
use gpui::{Context, ScrollEvent, Window, WindowOptions, div, rgb};

#[derive(Default)]
struct View {
    moves: usize,
    ends: usize,
}

impl Render for View {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("axis-stop-repro")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .on_scroll(cx.listener(|v, ev: &ScrollEvent, _, cx| {
                eprintln!("{:?} {:?}", ev.touch_phase, ev.delta);
                if ev.touch_phase == gpui::TouchPhase::Ended {
                    v.ends += 1;
                } else {
                    v.moves += 1;
                }
                cx.notify();
            }))
            .child(format!(
                "Move events: {} / end events: {}",
                self.moves, self.ends
            ))
    }
}

fn main() {
    gpui_platform::application().run(|cx| {
        let opts = WindowOptions {
            app_id: std::env::var("NT_APP_ID").ok(),
            focus: false,
            ..Default::default()
        };
        if let Err(err) = cx.open_window(opts, |_, cx| cx.new(|_| View::default())) {
            eprintln!("{err}");
            cx.quit();
        }
    });
}
