//! Design tokens from the spec, and the widget styles built from them.

use iced::widget::{button, container, text_input};
use iced::widget::image::Handle;
use iced::{Background, Border, Color, Font, Shadow, Theme, font};
use std::sync::OnceLock;

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const BG: Color = hex(0x0f0f0f);
pub const SURFACE: Color = hex(0x0f0f0f);
pub const RAISED: Color = hex(0x272727);
pub const HOVER: Color = hex(0x3f3f3f);
pub const SELECTED_BG: Color = hex(0xf1f1f1);
pub const SELECTED_TEXT: Color = hex(0x0f0f0f);
pub const TEXT: Color = hex(0xf1f1f1);
pub const TEXT_2: Color = hex(0xaaaaaa);
pub const DIVIDER: Color = hex(0x3f3f3f);
pub const ACCENT: Color = hex(0xff3b30);

pub const R_THUMB: f32 = 12.0;
pub const R_CHIP: f32 = 8.0;
pub const R_BUTTON: f32 = 18.0;

pub const TOP_BAR: f32 = 56.0;
pub const RAIL: f32 = 72.0;
pub const PAD: f32 = 24.0;

pub const MEDIUM: Font = Font { weight: font::Weight::Medium, ..Font::DEFAULT };
pub const BOLD: Font = Font { weight: font::Weight::Bold, ..Font::DEFAULT };

pub fn theme() -> Theme {
    Theme::custom(
        "NativeTube".to_string(),
        iced::theme::Palette { background: BG, text: TEXT, primary: ACCENT, success: ACCENT, warning: ACCENT, danger: ACCENT },
    )
}

/// 1x1 pixels stretched into badge backgrounds (see `widgets::Thumb`).
pub fn badge_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0u8, 0, 0, 204])).clone()
}
pub fn red_pixel() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| Handle::from_rgba(1, 1, vec![0xffu8, 0x3b, 0x30, 0xff])).clone()
}

fn bg(c: Color) -> Option<Background> {
    Some(Background::Color(c))
}

/// Raised pill button (Retry, Show more, Watch Later, ...).
pub fn pill(_: &Theme, status: button::Status) -> button::Style {
    let c = match status {
        button::Status::Hovered | button::Status::Pressed => HOVER,
        button::Status::Disabled => RAISED,
        button::Status::Active => RAISED,
    };
    button::Style {
        background: bg(c),
        text_color: if status == button::Status::Disabled { TEXT_2 } else { TEXT },
        border: Border { radius: R_BUTTON.into(), ..Border::default() },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Round icon button with no background until hovered.
pub fn icon_button(_: &Theme, status: button::Status) -> button::Style {
    let c = match status {
        button::Status::Hovered | button::Status::Pressed => bg(HOVER),
        _ => None,
    };
    button::Style {
        background: c,
        text_color: if status == button::Status::Disabled { hex(0x555555) } else { TEXT },
        border: Border { radius: R_BUTTON.into(), ..Border::default() },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Icon on the video controls.
pub fn control_button(_: &Theme, status: button::Status) -> button::Style {
    let c = match status {
        button::Status::Hovered | button::Status::Pressed => bg(Color::from_rgba(1.0, 1.0, 1.0, 0.15)),
        _ => None,
    };
    button::Style { background: c, text_color: TEXT, border: Border { radius: R_BUTTON.into(), ..Border::default() }, shadow: Shadow::default(), snap: true }
}

pub fn chip(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (b, tc) = if selected {
            (SELECTED_BG, SELECTED_TEXT)
        } else {
            (if matches!(status, button::Status::Hovered | button::Status::Pressed) { HOVER } else { RAISED }, TEXT)
        };
        button::Style { background: bg(b), text_color: tc, border: Border { radius: R_CHIP.into(), ..Border::default() }, shadow: Shadow::default(), snap: true }
    }
}

/// A whole card or list row: no background, a soft highlight on hover.
pub fn card(_: &Theme, status: button::Status) -> button::Style {
    let c = match status {
        button::Status::Hovered | button::Status::Pressed => bg(RAISED),
        _ => None,
    };
    button::Style { background: c, text_color: TEXT, border: Border { radius: R_THUMB.into(), ..Border::default() }, shadow: Shadow::default(), snap: true }
}

/// Text-only link (channel names): secondary, white on hover.
pub fn link(_: &Theme, status: button::Status) -> button::Style {
    let tc = match status {
        button::Status::Hovered | button::Status::Pressed => TEXT,
        _ => TEXT_2,
    };
    button::Style { background: None, text_color: tc, border: Border::default(), shadow: Shadow::default(), snap: true }
}

/// A transparent wrapper (avatars).
pub fn bare(_: &Theme, _: button::Status) -> button::Style {
    button::Style { background: None, text_color: TEXT, border: Border::default(), shadow: Shadow::default(), snap: true }
}

pub fn rail_entry(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let c = if selected {
            bg(RAISED)
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            bg(HOVER)
        } else {
            None
        };
        button::Style { background: c, text_color: TEXT, border: Border { radius: 10.0.into(), ..Border::default() }, shadow: Shadow::default(), snap: true }
    }
}

pub fn suggestion(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let c = if selected || matches!(status, button::Status::Hovered | button::Status::Pressed) { bg(HOVER) } else { None };
        button::Style { background: c, text_color: TEXT, border: Border::default(), shadow: Shadow::default(), snap: true }
    }
}

pub fn search_button(_: &Theme, status: button::Status) -> button::Style {
    let c = match status {
        button::Status::Hovered | button::Status::Pressed => HOVER,
        _ => hex(0x222222),
    };
    button::Style {
        background: bg(c),
        text_color: TEXT,
        border: Border {
            radius: iced::border::Radius { top_left: 0.0, bottom_left: 0.0, top_right: R_BUTTON, bottom_right: R_BUTTON },
            width: 1.0,
            color: DIVIDER,
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn search_input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(RAISED),
        border: Border {
            radius: iced::border::Radius { top_left: R_BUTTON, bottom_left: R_BUTTON, top_right: 0.0, bottom_right: 0.0 },
            width: 1.0,
            color: if focused { hex(0x1c62b9) } else { DIVIDER },
        },
        icon: TEXT_2,
        placeholder: TEXT_2,
        value: TEXT,
        selection: hex(0x3a5f8f),
    }
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style { background: bg(RAISED), border: Border { radius: R_THUMB.into(), ..Border::default() }, ..container::Style::default() }
}

pub fn dropdown(_: &Theme) -> container::Style {
    container::Style {
        background: bg(hex(0x212121)),
        border: Border { radius: R_THUMB.into(), width: 1.0, color: DIVIDER },
        shadow: Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.5), offset: iced::Vector::new(0.0, 4.0), blur_radius: 16.0 },
        ..container::Style::default()
    }
}

pub fn surface(_: &Theme) -> container::Style {
    container::Style { background: bg(SURFACE), text_color: Some(TEXT), ..container::Style::default() }
}

pub fn black(_: &Theme) -> container::Style {
    container::Style { background: bg(Color::BLACK), text_color: Some(TEXT), ..container::Style::default() }
}

pub fn controls_scrim(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Gradient(iced::Gradient::Linear(
            iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                .add_stop(0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0))
                .add_stop(1.0, Color::from_rgba(0.0, 0.0, 0.0, 0.75)),
        ))),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}
