//! Design tokens from docs/frontend-spec-v2.md ("Look and feel") and small text helpers.

use gpui::prelude::*;
use gpui::{Div, FontWeight, Hsla, Rgba, SharedString, div, px, rgb, rgba};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex_rgba: u32) -> Hsla {
    let c: Rgba = rgba(hex_rgba);
    c.into()
}

pub fn bg() -> Hsla {
    c(0x0f0f0f)
}
/// Dropdowns.
pub fn surface() -> Hsla {
    c(0x212121)
}
/// Menus.
pub fn menu_bg() -> Hsla {
    c(0x282828)
}
/// Chips, pills, inputs, description box: white at 10%.
pub fn fill() -> Hsla {
    ca(0xffffff1a)
}
/// White at 20%.
pub fn fill_hover() -> Hsla {
    ca(0xffffff33)
}
pub fn search_bg() -> Hsla {
    c(0x121212)
}
pub fn search_border() -> Hsla {
    c(0x303030)
}
pub fn search_focus() -> Hsla {
    c(0x1c62b9)
}
pub fn search_btn() -> Hsla {
    c(0x222222)
}
pub fn text() -> Hsla {
    c(0xf1f1f1)
}
pub fn text2() -> Hsla {
    c(0xaaaaaa)
}
pub fn on_light() -> Hsla {
    c(0x0f0f0f)
}
pub fn link() -> Hsla {
    c(0x3ea6ff)
}
pub fn divider() -> Hsla {
    ca(0xffffff33)
}
pub fn red() -> Hsla {
    c(0xff0033)
}
pub fn badge_bg() -> Hsla {
    ca(0x00000099)
}
pub fn black() -> Hsla {
    c(0x000000)
}

pub const RAIL_MIN_WINDOW: f32 = 792.0;
pub const SEARCH_COLLAPSE: f32 = 656.0;

pub const W400: FontWeight = FontWeight(400.0);
pub const W500: FontWeight = FontWeight(500.0);
pub const W700: FontWeight = FontWeight(700.0);

/// A text block in one of the spec's text styles: size / line height, weight, colour.
pub fn txt(s: impl Into<SharedString>, size: f32, lh: f32, weight: FontWeight, color: Hsla) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(lh))
        .font_weight(weight)
        .text_color(color)
        .child(s.into())
}

/// One line, cut with an ellipsis.
pub fn line(
    s: impl Into<SharedString>,
    size: f32,
    lh: f32,
    weight: FontWeight,
    color: Hsla,
) -> Div {
    txt(s, size, lh, weight, color)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
}

/// At most `n` lines, the last one ellipsized.
pub fn lines(
    s: impl Into<SharedString>,
    size: f32,
    lh: f32,
    weight: FontWeight,
    color: Hsla,
    n: usize,
) -> Div {
    txt(s, size, lh, weight, color)
        .line_clamp(n)
        .text_ellipsis()
}
