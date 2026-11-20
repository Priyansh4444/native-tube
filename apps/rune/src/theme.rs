//! Design tokens (spec v2), text styles, the bundled Roboto faces, the shared Material Symbols
//! icons and the rune-kit theme.

use rune::{Color, Icon, TextStyle};
use std::sync::LazyLock;

pub struct Tok {
    pub bg: Color,
    /// Dropdowns (suggestions).
    pub surface: Color,
    /// Menus (settings, card menus).
    pub menu: Color,
    /// Chips, pills, inputs, the description box: white at 10 %.
    pub fill: Color,
    /// Hover over a fill: white at 20 %.
    pub fill_hover: Color,
    pub field_bg: Color,
    pub field_border: Color,
    pub field_focus: Color,
    pub button_bg: Color,
    pub chip_sel_bg: Color,
    pub chip_sel_fg: Color,
    pub text: Color,
    pub text2: Color,
    pub link: Color,
    pub divider: Color,
    pub red: Color,
    pub badge: Color,
    /// Skeleton placeholders.
    pub skeleton: Color,
    pub hover_tint: Color,
}

fn hex(s: &str) -> Color {
    rune::color::hex(s).unwrap_or(Color::BLACK)
}

static TOK: LazyLock<Tok> = LazyLock::new(|| Tok {
    bg: hex("#0f0f0f"),
    surface: hex("#212121"),
    menu: hex("#282828"),
    fill: hex("#ffffff1a"),
    fill_hover: hex("#ffffff33"),
    field_bg: hex("#121212"),
    field_border: hex("#303030"),
    field_focus: hex("#1c62b9"),
    button_bg: hex("#222222"),
    chip_sel_bg: hex("#f1f1f1"),
    chip_sel_fg: hex("#0f0f0f"),
    text: hex("#f1f1f1"),
    text2: hex("#aaaaaa"),
    link: hex("#3ea6ff"),
    divider: hex("#ffffff33"),
    red: hex("#ff0033"),
    badge: hex("#00000099"),
    skeleton: hex("#272727"),
    hover_tint: hex(HOVER_TINT),
});

pub fn t() -> &'static Tok {
    &TOK
}

// The spec's text styles.
pub const S10: TextStyle = TextStyle::new(10.0, 400.0).lh(14.0);
pub const S12: TextStyle = TextStyle::new(12.0, 400.0).lh(18.0);
pub const S12M: TextStyle = TextStyle::new(12.0, 500.0).lh(18.0);
pub const S13M: TextStyle = TextStyle::new(13.0, 500.0).lh(18.0);
pub const S13_TNUM: TextStyle = TextStyle::new(13.0, 400.0).lh(18.0).features("'tnum' 1");
pub const S14: TextStyle = TextStyle::new(14.0, 400.0).lh(20.0);
pub const S14M: TextStyle = TextStyle::new(14.0, 500.0).lh(20.0);
pub const S16: TextStyle = TextStyle::new(16.0, 400.0).lh(22.0);
pub const S16M: TextStyle = TextStyle::new(16.0, 500.0).lh(22.0);
pub const S18: TextStyle = TextStyle::new(18.0, 400.0).lh(26.0);
pub const S20B: TextStyle = TextStyle::new(20.0, 700.0).lh(28.0);
pub const S36B: TextStyle = TextStyle::new(36.0, 700.0).lh(44.0);

pub const TOP_BAR: f32 = 56.0;
pub const RAIL: f32 = 72.0;
pub const CHIP_BAR: f32 = 56.0;
/// The full guide (☰).
pub const GUIDE_W: f32 = 240.0;
/// The card hover tint (YouTube's #272727 box behind a hovered card).
pub const HOVER_TINT: &str = "#272727";

/// Roboto, embedded (assets/fonts, Apache-2.0). Never a system font.
pub static FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/Roboto-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Roboto-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Roboto-Bold.ttf"),
];
pub const FAMILIES: (&str, &str) = ("Roboto", "Roboto");

/// rune-kit's theme from the spec's tokens.
pub fn kit() -> &'static rune_kit::Kit {
    static KIT: LazyLock<rune_kit::Kit> = LazyLock::new(|| {
        let tok = t();
        let mut k = rune_kit::Kit::from_palette(rune_kit::PaletteSeed {
            bg: tok.bg,
            fg: tok.text,
            accent: tok.red,
        });
        k.radius.xl = 18.0;
        k.c.bg_button_neutral = tok.fill;
        k.c.hover = hex("#ffffff1a");
        k.c.bg_contrast = tok.chip_sel_bg;
        k.c.text_contrast = tok.chip_sel_fg;
        k.c.border_focus = tok.text;
        k.elevation.button_neutral = Vec::new();
        k.elevation.button_contrast = Vec::new();
        k
    });
    &KIT
}

/// The search field's kit: no fill of its own (the field's box draws it), no focus ring (the
/// box's border turns blue instead).
pub fn input_kit() -> &'static rune_kit::Kit {
    static KIT: LazyLock<rune_kit::Kit> = LazyLock::new(|| {
        let mut k = kit().clone();
        k.c.bg_base = Color::TRANSPARENT;
        k
    });
    &KIT
}

/// Material Symbols Outlined (assets/icons): their viewBox is `0 -960 960 960`, so the body is
/// moved down by 960 to put it in Rune's `0 0 view view` box. The paths are black; `icon(..)
/// .color(..)` tints them.
macro_rules! symbol {
    ($name:ident, $file:literal) => {
        pub static $name: Icon = Icon {
            name: concat!("ms-", $file),
            view: 960.0,
            body: concat!(
                "<g transform=\"translate(0 960)\">",
                include_str!(concat!("../../../assets/icons/", $file, ".svg")),
                "</g>"
            ),
        };
    };
}

symbol!(BACK, "arrow_back");
symbol!(SEARCH, "search");
symbol!(HOME, "home");
symbol!(HOME_FILL, "home_fill1");
symbol!(SUBS, "subscriptions");
symbol!(SUBS_FILL, "subscriptions_fill1");
symbol!(HISTORY, "history");
symbol!(HISTORY_FILL, "history_fill1");
symbol!(LATER, "schedule");
symbol!(LATER_FILL, "schedule_fill1");
symbol!(MORE, "more_vert");
symbol!(PLAY, "play_arrow_fill1");
symbol!(PLAY_OUTLINE, "play_arrow");
symbol!(PAUSE, "pause_fill1");
symbol!(REPLAY, "replay");
symbol!(NEXT, "skip_next_fill1");
symbol!(VOLUME, "volume_up_fill1");
symbol!(VOLUME_LOW, "volume_down_fill1");
symbol!(MUTED, "volume_off_fill1");
symbol!(SETTINGS, "settings");
symbol!(THEATRE, "width_wide");
symbol!(FULLSCREEN, "fullscreen");
symbol!(FULLSCREEN_EXIT, "fullscreen_exit");
symbol!(LIKE, "thumb_up");
symbol!(LIKE_FILL, "thumb_up_fill1");
symbol!(DISLIKE, "thumb_down");
symbol!(DISLIKE_FILL, "thumb_down_fill1");
symbol!(SHARE, "share");
symbol!(BELL, "notifications");
symbol!(CHEVRON_LEFT, "chevron_left");
symbol!(CHEVRON_RIGHT, "chevron_right");
symbol!(CHECK, "check");
symbol!(ERROR, "error");
symbol!(OFFLINE, "wifi_off");
symbol!(LIBRARY, "video_library");
symbol!(DELETE, "delete");
symbol!(CC, "closed_caption");
symbol!(CC_FILL, "closed_caption_fill1");
symbol!(CLOSE, "close");

/// Material Symbols that assets/icons doesn't carry, inline (same viewBox convention).
macro_rules! inline_symbol {
    ($name:ident, $id:literal, $d:literal) => {
        pub static $name: Icon = Icon {
            name: concat!("ms-inline-", $id),
            view: 960.0,
            body: concat!(
                "<g transform=\"translate(0 960)\"><path d=\"",
                $d,
                "\"/></g>"
            ),
        };
    };
}

inline_symbol!(
    MENU,
    "menu",
    "M120-240v-80h720v80H120Zm0-200v-80h720v80H120Zm0-200v-80h720v80H120Z"
);
inline_symbol!(
    INFO,
    "info",
    "M440-280h80v-240h-80v240Zm40-320q17 0 28.5-11.5T520-640q0-17-11.5-28.5T480-680q-17 0-28.5 11.5T440-640q0 17 11.5 28.5T480-600Zm0 520q-83 0-156-31.5T197-197q-54-54-85.5-127T80-480q0-83 31.5-156T197-763q54-54 127-85.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 83-31.5 156T763-197q-54 54-127 85.5T480-80Zm0-80q134 0 227-93t93-227q0-134-93-227t-227-93q-134 0-227 93t-93 227q0 134 93 227t227 93Z"
);
inline_symbol!(
    LOOP,
    "repeat",
    "M280-80 120-240l160-160 56 58-62 62h406v-160h80v240H274l62 62-56 58Zm-80-440v-240h486l-62-62 56-58 160 160-160 160-56-58 62-62H280v160h-80Z"
);
