//! The bare iced shell: one window, one label.

use iced::widget::{center, text};
use iced::{Color, Element, Font, Size, window};

const ROBOTO: Font = Font::with_name("Roboto");

fn view(_: &()) -> Element<'_, ()> {
    center(
        text("NativeTube")
            .size(14)
            .font(ROBOTO)
            .color(Color::from_rgb8(0xf1, 0xf1, 0xf1)),
    )
    .into()
}

fn main() -> iced::Result {
    let window = window::Settings {
        size: Size::new(1280.0, 800.0),
        platform_specific: window::settings::PlatformSpecific {
            #[cfg(target_os = "linux")]
            application_id: std::env::var("NT_APP_ID")
                .unwrap_or_else(|_| "nt-shell-iced".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    iced::application(|| (), |_: &mut (), _: ()| {}, view)
        .title("NativeTube shell")
        .font(include_bytes!("../../../assets/fonts/Roboto-Regular.ttf").as_slice())
        .style(|_, _| iced::theme::Style {
            background_color: Color::from_rgb8(0x0f, 0x0f, 0x0f),
            text_color: Color::from_rgb8(0xf1, 0xf1, 0xf1),
        })
        .window(window)
        .antialiasing(false)
        .run()
}
