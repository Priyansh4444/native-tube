//! The shared Material Symbols icons (`assets/icons`), embedded, served to GPUI's `svg()` under
//! `icons/nt/<name>.svg`. GPUI draws an SVG as a mask, so `text_color` tints it. The kit's own
//! components still get their icons from `gpui_kit_assets`.

use gpui::prelude::*;
use gpui::{AssetSource, Hsla, SharedString, Svg, px, svg};
use std::borrow::Cow;

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/nt/arrow_back.svg",
        include_bytes!("../../../assets/icons/arrow_back.svg"),
    ),
    (
        "icons/nt/bookmark.svg",
        include_bytes!("../../../assets/icons/bookmark.svg"),
    ),
    (
        "icons/nt/check.svg",
        include_bytes!("../../../assets/icons/check.svg"),
    ),
    (
        "icons/nt/chevron_left.svg",
        include_bytes!("../../../assets/icons/chevron_left.svg"),
    ),
    (
        "icons/nt/chevron_right.svg",
        include_bytes!("../../../assets/icons/chevron_right.svg"),
    ),
    (
        "icons/nt/close.svg",
        include_bytes!("../../../assets/icons/close.svg"),
    ),
    (
        "icons/nt/delete.svg",
        include_bytes!("../../../assets/icons/delete.svg"),
    ),
    (
        "icons/nt/error.svg",
        include_bytes!("../../../assets/icons/error.svg"),
    ),
    (
        "icons/nt/expand_less.svg",
        include_bytes!("../../../assets/icons/expand_less.svg"),
    ),
    (
        "icons/nt/expand_more.svg",
        include_bytes!("../../../assets/icons/expand_more.svg"),
    ),
    (
        "icons/nt/fullscreen.svg",
        include_bytes!("../../../assets/icons/fullscreen.svg"),
    ),
    (
        "icons/nt/fullscreen_exit.svg",
        include_bytes!("../../../assets/icons/fullscreen_exit.svg"),
    ),
    (
        "icons/nt/history.svg",
        include_bytes!("../../../assets/icons/history.svg"),
    ),
    (
        "icons/nt/home.svg",
        include_bytes!("../../../assets/icons/home.svg"),
    ),
    (
        "icons/nt/more_vert.svg",
        include_bytes!("../../../assets/icons/more_vert.svg"),
    ),
    (
        "icons/nt/notifications.svg",
        include_bytes!("../../../assets/icons/notifications.svg"),
    ),
    (
        "icons/nt/pause.svg",
        include_bytes!("../../../assets/icons/pause.svg"),
    ),
    (
        "icons/nt/play_arrow.svg",
        include_bytes!("../../../assets/icons/play_arrow.svg"),
    ),
    (
        "icons/nt/replay.svg",
        include_bytes!("../../../assets/icons/replay.svg"),
    ),
    (
        "icons/nt/schedule.svg",
        include_bytes!("../../../assets/icons/schedule.svg"),
    ),
    (
        "icons/nt/search.svg",
        include_bytes!("../../../assets/icons/search.svg"),
    ),
    (
        "icons/nt/settings.svg",
        include_bytes!("../../../assets/icons/settings.svg"),
    ),
    (
        "icons/nt/share.svg",
        include_bytes!("../../../assets/icons/share.svg"),
    ),
    (
        "icons/nt/skip_next.svg",
        include_bytes!("../../../assets/icons/skip_next.svg"),
    ),
    (
        "icons/nt/speed.svg",
        include_bytes!("../../../assets/icons/speed.svg"),
    ),
    (
        "icons/nt/subscriptions.svg",
        include_bytes!("../../../assets/icons/subscriptions.svg"),
    ),
    (
        "icons/nt/thumb_down.svg",
        include_bytes!("../../../assets/icons/thumb_down.svg"),
    ),
    (
        "icons/nt/thumb_up.svg",
        include_bytes!("../../../assets/icons/thumb_up.svg"),
    ),
    (
        "icons/nt/tune.svg",
        include_bytes!("../../../assets/icons/tune.svg"),
    ),
    (
        "icons/nt/video_library.svg",
        include_bytes!("../../../assets/icons/video_library.svg"),
    ),
    (
        "icons/nt/volume_down.svg",
        include_bytes!("../../../assets/icons/volume_down.svg"),
    ),
    (
        "icons/nt/volume_off.svg",
        include_bytes!("../../../assets/icons/volume_off.svg"),
    ),
    (
        "icons/nt/volume_up.svg",
        include_bytes!("../../../assets/icons/volume_up.svg"),
    ),
    (
        "icons/nt/width_wide.svg",
        include_bytes!("../../../assets/icons/width_wide.svg"),
    ),
    (
        "icons/nt/wifi_off.svg",
        include_bytes!("../../../assets/icons/wifi_off.svg"),
    ),
    (
        "icons/nt/playlist_add.svg",
        include_bytes!("../../../assets/icons/playlist_add.svg"),
    ),
    (
        "icons/nt/keyboard_arrow_down.svg",
        include_bytes!("../../../assets/icons/keyboard_arrow_down.svg"),
    ),
    (
        "icons/nt/home_fill1.svg",
        include_bytes!("../../../assets/icons/home_fill1.svg"),
    ),
    (
        "icons/nt/subscriptions_fill1.svg",
        include_bytes!("../../../assets/icons/subscriptions_fill1.svg"),
    ),
    (
        "icons/nt/history_fill1.svg",
        include_bytes!("../../../assets/icons/history_fill1.svg"),
    ),
    (
        "icons/nt/schedule_fill1.svg",
        include_bytes!("../../../assets/icons/schedule_fill1.svg"),
    ),
    (
        "icons/nt/bookmark_fill1.svg",
        include_bytes!("../../../assets/icons/bookmark_fill1.svg"),
    ),
    (
        "icons/nt/thumb_up_fill1.svg",
        include_bytes!("../../../assets/icons/thumb_up_fill1.svg"),
    ),
];

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = ICONS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut v: Vec<SharedString> = ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect();
        v.extend(gpui_kit_assets::Assets.list(path)?);
        Ok(v)
    }
}

/// A Material Symbols icon, `size` px, tinted `color`. `name` is the file stem, e.g. "home" or
/// "home_fill1".
pub fn icon(name: &'static str, size: f32, color: Hsla) -> Svg {
    // The static path from the table, so drawing an icon allocates nothing.
    let path = ICONS
        .iter()
        .map(|(p, _)| *p)
        .find(|p| {
            p.strip_prefix("icons/nt/")
                .and_then(|s| s.strip_suffix(".svg"))
                == Some(name)
        })
        .unwrap_or("");
    svg()
        .path(SharedString::from(path))
        .size(px(size))
        .flex_none()
        .text_color(color)
}
