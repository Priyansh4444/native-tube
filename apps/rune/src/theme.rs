//! Design tokens from the spec, text styles, the app's own icons and the rune-kit theme.

use rune::{Color, Icon, TextStyle};
use std::sync::LazyLock;

pub struct Tok {
    pub bg: Color,
    pub raised: Color,
    pub hover: Color,
    pub chip_sel_bg: Color,
    pub chip_sel_fg: Color,
    pub text: Color,
    pub text2: Color,
    pub accent: Color,
    pub badge: Color,
    /// A light wash for hovered cards and rows.
    pub wash: Color,
}

fn hex(s: &str) -> Color {
    rune::color::hex(s).unwrap_or(Color::BLACK)
}

static TOK: LazyLock<Tok> = LazyLock::new(|| Tok {
    bg: hex("#0f0f0f"),
    raised: hex("#272727"),
    hover: hex("#3f3f3f"),
    chip_sel_bg: hex("#f1f1f1"),
    chip_sel_fg: hex("#0f0f0f"),
    text: hex("#f1f1f1"),
    text2: hex("#aaaaaa"),
    accent: hex("#ff3b30"),
    badge: hex("#000000cc"),
    wash: hex("#ffffff12"),
});

pub fn t() -> &'static Tok {
    &TOK
}

pub const S12: TextStyle = TextStyle::new(12.0, 400.0).lh(18.0);
pub const S12M: TextStyle = TextStyle::new(12.0, 500.0).lh(18.0);
pub const S14: TextStyle = TextStyle::new(14.0, 400.0).lh(20.0);
pub const S14M: TextStyle = TextStyle::new(14.0, 500.0).lh(20.0);
pub const S14B: TextStyle = TextStyle::new(14.0, 600.0).lh(20.0);
pub const S16M: TextStyle = TextStyle::new(16.0, 500.0).lh(22.0);
pub const S18: TextStyle = TextStyle::new(18.0, 400.0).lh(26.0);
pub const S20: TextStyle = TextStyle::new(20.0, 600.0).lh(28.0);
pub const S20B: TextStyle = TextStyle::new(20.0, 700.0).lh(28.0);
pub const S12_TNUM: TextStyle = TextStyle::new(12.0, 500.0).lh(18.0).features("'tnum' 1");

pub const PAD: f32 = 24.0;
pub const TOP_BAR: f32 = 56.0;
pub const RAIL: f32 = 72.0;

/// rune-kit's theme from the spec's tokens: flat raised controls, pill buttons, red accent.
pub fn kit() -> &'static rune_kit::Kit {
    static KIT: LazyLock<rune_kit::Kit> = LazyLock::new(|| {
        let tok = t();
        let mut k = rune_kit::Kit::from_palette(rune_kit::PaletteSeed {
            bg: tok.bg,
            fg: tok.text,
            accent: tok.accent,
        });
        k.radius.xl = 18.0;
        k.c.bg_button_neutral = tok.raised;
        k.c.bg_contrast = tok.chip_sel_bg;
        k.c.text_contrast = tok.chip_sel_fg;
        k.elevation.button_neutral = Vec::new();
        k.elevation.button_contrast = Vec::new();
        k
    });
    &KIT
}

macro_rules! icon {
    ($name:ident, $id:literal, $body:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: $body,
        };
    };
}

icon!(
    BACK,
    "nt-back",
    r#"<path d="M20 12H5M11 5L4 12L11 19" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#
);
icon!(
    SEARCH,
    "nt-search",
    r#"<circle cx="10.5" cy="10.5" r="6.5" stroke="currentColor" stroke-width="2" fill="none"/><path d="M15.5 15.5L21 21" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#
);
icon!(
    HOME,
    "nt-home",
    r#"<path d="M4 10.5L12 4L20 10.5V20H14.5V14.5H9.5V20H4Z" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linejoin="round"/>"#
);
icon!(
    SUBS,
    "nt-subs",
    r#"<rect x="3" y="8" width="18" height="13" rx="2" stroke="currentColor" stroke-width="1.8" fill="none"/><path d="M6 5H18M8 2.5H16" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M10 11.5V17.5L15 14.5Z" fill="currentColor"/>"#
);
icon!(
    HISTORY,
    "nt-history",
    r#"<circle cx="12" cy="12" r="8.5" stroke="currentColor" stroke-width="1.8" fill="none"/><path d="M12 7V12L15.5 14" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>"#
);
icon!(
    LATER,
    "nt-later",
    r#"<path d="M6 3.5H18V21L12 16.5L6 21Z" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linejoin="round"/>"#
);
icon!(
    SAVED,
    "nt-saved",
    r#"<path d="M6 3.5H18V21L12 16.5L6 21Z" fill="currentColor"/>"#
);
icon!(
    PLAY,
    "nt-play",
    r#"<path d="M7 4.5V19.5L19.5 12Z" fill="currentColor"/>"#
);
icon!(
    PAUSE,
    "nt-pause",
    r#"<rect x="6" y="4.5" width="4" height="15" rx="1" fill="currentColor"/><rect x="14" y="4.5" width="4" height="15" rx="1" fill="currentColor"/>"#
);
icon!(
    VOLUME,
    "nt-volume",
    r#"<path d="M4 9.5H8L13 5V19L8 14.5H4Z" fill="currentColor"/><path d="M16 8.5C17.3 10.3 17.3 13.7 16 15.5M18.5 6C21 9.5 21 14.5 18.5 18" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>"#
);
icon!(
    MUTED,
    "nt-muted",
    r#"<path d="M4 9.5H8L13 5V19L8 14.5H4Z" fill="currentColor"/><path d="M16 9L21 15M21 9L16 15" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#
);
icon!(
    LIKE,
    "nt-like",
    r#"<path d="M7 10.5V20H4V10.5ZM7 10.5L11 3.5C12.5 3.5 13.5 4.5 13 6.5L12.3 9.5H18.5C19.8 9.5 20.6 10.7 20.2 11.9L18.2 18.6C17.9 19.4 17.2 20 16.3 20H7" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/>"#
);

/// The kit for the search field: a raised pill.
pub fn input_kit() -> &'static rune_kit::Kit {
    static KIT: LazyLock<rune_kit::Kit> = LazyLock::new(|| {
        let mut k = kit().clone();
        k.c.bg_base = t().raised;
        k.radius.md = 20.0;
        k
    });
    &KIT
}
