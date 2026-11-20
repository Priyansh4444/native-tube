//! Design tokens (docs/frontend-spec-v2.md, "Design tokens"), the bundled Roboto faces, the
//! text styles, and the widget styles built from them.

use iced::widget::image::Handle;
use iced::widget::{button, container, slider, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, font};
use std::sync::OnceLock;

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
const fn white(a: f32) -> Color {
    Color::from_rgba(1.0, 1.0, 1.0, a)
}

/// iced blends in linear light; a browser blends sRGB values. These convert a CSS alpha to the
/// alpha that gives the same visible result: white over dark, and black over a picture.
pub fn css_white(a: f32) -> Color {
    white(a.clamp(0.0, 1.0).powf(2.2))
}
pub fn css_black(a: f32) -> Color {
    Color::from_rgba(0.0, 0.0, 0.0, 1.0 - (1.0 - a.clamp(0.0, 1.0)).powf(2.2))
}

pub const BG: Color = hex(0x0f0f0f);
pub const DROPDOWN: Color = hex(0x212121);
pub const MENU: Color = hex(0x282828);
/// Chips, pills, inputs, the description box: the spec's white at 10% as it looks on the
/// background (#272727). Drawn opaque: iced blends in linear light on an sRGB surface, so a
/// translucent white comes out much lighter than in a browser.
pub const FILL: Color = hex(0x272727);
pub const FILL_HOVER: Color = hex(0x3f3f3f);
/// The same fills, opaque, for the few places drawn over images or under other layers.
pub const FILL_SOLID: Color = hex(0x272727);
pub const SEARCH_BG: Color = hex(0x121212);
pub const SEARCH_BORDER: Color = hex(0x303030);
pub const SEARCH_FOCUS: Color = hex(0x1c62b9);
pub const SEARCH_BUTTON: Color = hex(0x222222);
pub const SELECTED_BG: Color = hex(0xf1f1f1);
pub const SELECTED_TEXT: Color = hex(0x0f0f0f);
pub const TEXT: Color = hex(0xf1f1f1);
pub const TEXT_2: Color = hex(0xaaaaaa);
pub const LINK: Color = hex(0x3ea6ff);
pub const DIVIDER: Color = hex(0x3f3f3f);
pub const RED: Color = hex(0xff0033);

pub const R_THUMB: f32 = 12.0;
pub const R_COMPACT: f32 = 8.0;
pub const R_CHIP: f32 = 8.0;
pub const R_PILL: f32 = 18.0;
pub const R_MENU: f32 = 12.0;

pub const TOP_BAR: f32 = 56.0;
pub const RAIL: f32 = 72.0;
pub const CHIP_BAR: f32 = 56.0;

// ---- fonts ------------------------------------------------------------------------------------

pub const ROBOTO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Regular.ttf");
pub const ROBOTO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Medium.ttf");
pub const ROBOTO_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Bold.ttf");

pub const REGULAR: Font = Font::with_name("Roboto");
pub const MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..REGULAR
};
pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..REGULAR
};

/// A text style from the spec's table: size / line height and weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub size: f32,
    pub line: f32,
    pub font: Font,
}

const fn st(size: f32, line: f32, font: Font) -> Style {
    Style { size, line, font }
}

pub const RAIL_LABEL: Style = st(10.0, 14.0, REGULAR);
pub const META_S: Style = st(12.0, 18.0, REGULAR);
pub const META_S_M: Style = st(12.0, 18.0, MEDIUM);
pub const AUTHOR: Style = st(13.0, 18.0, MEDIUM);
pub const BODY: Style = st(14.0, 20.0, REGULAR);
pub const BODY_M: Style = st(14.0, 20.0, MEDIUM);
pub const SUGGEST: Style = st(16.0, 22.0, REGULAR);
pub const CARD_TITLE: Style = st(16.0, 22.0, MEDIUM);
pub const TAB: Style = st(16.0, 22.0, MEDIUM);
pub const SEARCH_TITLE: Style = st(18.0, 26.0, REGULAR);
pub const HEADING: Style = st(20.0, 28.0, BOLD);
pub const CHANNEL_NAME: Style = st(36.0, 44.0, BOLD);
pub const TIME: Style = st(13.0, 18.0, REGULAR);

pub fn theme() -> Theme {
    Theme::custom(
        "NativeTube".to_string(),
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: SEARCH_FOCUS,
            success: RED,
            warning: RED,
            danger: RED,
        },
    )
}

/// 1x1 pixel stretched into duration-badge backgrounds (see `widgets::Thumb`).
pub fn badge_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0u8, 0, 0, 221]))
        .clone()
}
pub fn red_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0xffu8, 0x00, 0x33, 0xff]))
        .clone()
}
/// 1x1 black at 10%, laid over a thumbnail while it is pressed.
pub fn press_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0u8, 0, 0, 53]))
        .clone()
}
/// 1x1 white at 30%, the unwatched part of the resume bar.
pub fn track_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0x77u8, 0x77, 0x77, 0xb3]))
        .clone()
}

fn bg(c: Color) -> Option<Background> {
    Some(Background::Color(c))
}

fn hovered(status: button::Status) -> bool {
    matches!(status, button::Status::Hovered | button::Status::Pressed)
}

fn plain(background: Option<Background>, text: Color, radius: f32) -> button::Style {
    button::Style {
        background,
        text_color: text,
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Fill pill (Watch Later, Share, Retry, Subscribed, menus' buttons).
pub fn pill(_: &Theme, status: button::Status) -> button::Style {
    let c = if hovered(status) { FILL_HOVER } else { FILL };
    plain(bg(c), TEXT, R_PILL)
}

/// The left or right half of the like | dislike pill.
pub fn pill_half(left: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let c = if hovered(status) { FILL_HOVER } else { FILL };
        let r = iced::border::Radius {
            top_left: if left { R_PILL } else { 0.0 },
            bottom_left: if left { R_PILL } else { 0.0 },
            top_right: if left { 0.0 } else { R_PILL },
            bottom_right: if left { 0.0 } else { R_PILL },
        };
        button::Style {
            border: Border {
                radius: r,
                ..Border::default()
            },
            ..plain(bg(c), TEXT, 0.0)
        }
    }
}

/// The primary button (Subscribe): light on dark.
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let c = if hovered(status) {
        hex(0xd9d9d9)
    } else {
        SELECTED_BG
    };
    plain(bg(c), SELECTED_TEXT, R_PILL)
}

/// Round icon button with no background until hovered.
pub fn icon_button(_: &Theme, status: button::Status) -> button::Style {
    let c = if hovered(status) { bg(FILL) } else { None };
    let tc = if status == button::Status::Disabled {
        hex(0x555555)
    } else {
        TEXT
    };
    plain(c, tc, 20.0)
}

/// Icons on the video controls: no background, slight highlight.
pub fn control_button(_: &Theme, status: button::Status) -> button::Style {
    let c = if hovered(status) {
        bg(css_white(0.1))
    } else {
        None
    };
    plain(c, TEXT, 20.0)
}

pub fn chip(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        if selected {
            plain(bg(SELECTED_BG), SELECTED_TEXT, R_CHIP)
        } else {
            let c = if hovered(status) { FILL_HOVER } else { FILL };
            plain(bg(c), TEXT, R_CHIP)
        }
    }
}

/// A whole card or list row: no background, ever (YouTube shows "⋮" and the hand instead).
pub fn card(_: &Theme, _: button::Status) -> button::Style {
    plain(None, TEXT, R_THUMB)
}

/// Text-only link (channel names): secondary, white on hover.
pub fn link(_: &Theme, status: button::Status) -> button::Style {
    plain(None, if hovered(status) { TEXT } else { TEXT_2 }, 0.0)
}

/// A transparent wrapper (avatars, titles).
pub fn bare(_: &Theme, _: button::Status) -> button::Style {
    plain(None, TEXT, 0.0)
}

pub fn menu_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let c = if selected || hovered(status) {
            bg(FILL)
        } else {
            None
        };
        plain(c, TEXT, 0.0)
    }
}

pub fn search_button(_: &Theme, status: button::Status) -> button::Style {
    let c = if hovered(status) {
        hex(0x303030)
    } else {
        SEARCH_BUTTON
    };
    button::Style {
        border: Border {
            radius: iced::border::Radius {
                top_left: 0.0,
                bottom_left: 0.0,
                top_right: 20.0,
                bottom_right: 20.0,
            },
            width: 1.0,
            color: SEARCH_BORDER,
        },
        ..plain(bg(c), TEXT, 0.0)
    }
}

pub fn search_input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            radius: 0.0.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        icon: TEXT_2,
        placeholder: hex(0x888888),
        value: TEXT,
        selection: if focused {
            hex(0x264f78)
        } else {
            hex(0x3a3a3a)
        },
    }
}

/// The search field's frame (the input itself is borderless inside it).
pub fn search_frame(focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: bg(SEARCH_BG),
        border: Border {
            radius: iced::border::Radius {
                top_left: 20.0,
                bottom_left: 20.0,
                top_right: 0.0,
                bottom_right: 0.0,
            },
            width: 1.0,
            color: if focused { SEARCH_FOCUS } else { SEARCH_BORDER },
        },
        ..container::Style::default()
    }
}

pub fn fill_box(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: bg(FILL),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

pub fn dropdown(_: &Theme) -> container::Style {
    container::Style {
        background: bg(DROPDOWN),
        border: Border {
            radius: R_MENU.into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..container::Style::default()
    }
}

pub fn menu(_: &Theme) -> container::Style {
    container::Style {
        background: bg(MENU),
        border: Border {
            radius: R_MENU.into(),
            ..Border::default()
        },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn surface(_: &Theme) -> container::Style {
    container::Style {
        background: bg(BG),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn black(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: bg(Color::BLACK),
        text_color: Some(TEXT),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

pub fn scrim(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Gradient(iced::Gradient::Linear(
            iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                .add_stop(0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0))
                .add_stop(1.0, css_black(0.6)),
        ))),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn overlay_card(_: &Theme) -> container::Style {
    container::Style {
        background: bg(css_black(0.75)),
        border: Border {
            radius: R_MENU.into(),
            ..Border::default()
        },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn divider(_: &Theme) -> container::Style {
    container::Style {
        background: bg(DIVIDER),
        ..container::Style::default()
    }
}

pub fn underline(_: &Theme) -> container::Style {
    container::Style {
        background: bg(TEXT),
        ..container::Style::default()
    }
}

pub fn volume(_: &Theme, _: slider::Status) -> slider::Style {
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(TEXT), Background::Color(css_white(0.2))),
            width: 3.0,
            border: Border {
                radius: 1.5.into(),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.0 },
            background: Background::Color(TEXT),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}
