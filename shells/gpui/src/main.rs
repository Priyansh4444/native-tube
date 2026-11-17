//! The bare GPUI shell: one window, one label.

use gpui::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, point, px, rgb, size,
};
use std::borrow::Cow;

struct Shell;

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x0f0f0f))
            .text_color(rgb(0xf1f1f1))
            .font_family("Roboto")
            .text_size(px(14.0))
            .child("NativeTube")
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let font = include_bytes!("../../../assets/fonts/Roboto-Regular.ttf");
        if let Err(e) = cx
            .text_system()
            .add_fonts(vec![Cow::Borrowed(font.as_slice())])
        {
            eprintln!("shell-gpui: can't load Roboto: {e}");
        }
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(1280.0), px(800.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("NativeTube shell".into()),
                ..Default::default()
            }),
            app_id: Some(
                std::env::var("NT_APP_ID").unwrap_or_else(|_| "nt-shell-gpui".to_string()),
            ),
            focus: false,
            ..Default::default()
        };
        if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| Shell)) {
            eprintln!("shell-gpui: can't open a window: {e}");
            cx.quit();
        }
    });
}
