//! The spec's design tokens, as GPUI colours and sizes.

use gpui::{Hsla, Rgba, rgb, rgba};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn bg() -> Hsla {
    c(0x0f0f0f)
}
pub fn raised() -> Hsla {
    c(0x272727)
}
pub fn hover() -> Hsla {
    c(0x3f3f3f)
}
pub fn chip_selected_bg() -> Hsla {
    c(0xf1f1f1)
}
pub fn chip_selected_fg() -> Hsla {
    c(0x0f0f0f)
}
pub fn text() -> Hsla {
    c(0xf1f1f1)
}
pub fn text2() -> Hsla {
    c(0xaaaaaa)
}
pub fn divider() -> Hsla {
    c(0x3f3f3f)
}
pub fn accent() -> Hsla {
    c(0xff3b30)
}
pub fn badge_bg() -> Hsla {
    let c: Rgba = rgba(0x000000cc);
    c.into()
}

pub const TOP_BAR_H: f32 = 56.0;
pub const RAIL_W: f32 = 72.0;
pub const PAD: f32 = 24.0;
pub const CHIPS_H: f32 = 56.0;

pub const R_THUMB: f32 = 12.0;
pub const R_CHIP: f32 = 8.0;
pub const R_BUTTON: f32 = 18.0;
