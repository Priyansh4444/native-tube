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
    (
        "icons/nt/closed_caption.svg",
        include_bytes!("../../../assets/icons/closed_caption.svg"),
    ),
    (
        "icons/nt/closed_caption_fill1.svg",
        include_bytes!("../../../assets/icons/closed_caption_fill1.svg"),
    ),
    // Not in the shared set: Material Icons (Apache-2.0) path data.
    ("icons/nt/favorite.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54z"/></svg>"#),
    ("icons/nt/skip_previous.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M6 6h2v12H6zm3.5 6L18 6v12z"/></svg>"#),
    ("icons/nt/repeat.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M7 7h10v3l4-4-4-4v3H5v6h2V7zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4z"/></svg>"#),
    ("icons/nt/menu.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z"/></svg>"#),
    ("icons/nt/timer.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M15 1H9v2h6V1zm-4 13h2V8h-2v6zm8.03-6.61l1.42-1.42c-.43-.51-.9-.99-1.41-1.41l-1.42 1.42C16.07 4.74 14.12 4 12 4c-4.97 0-9 4.03-9 9s4.02 9 9 9 9-4.03 9-9c0-2.12-.74-4.07-1.97-5.61zM12 20c-3.87 0-7-3.13-7-7s3.13-7 7-7 7 3.13 7 7-3.13 7-7 7z"/></svg>"#),
    ("icons/nt/link.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3.9 12c0-1.71 1.39-3.1 3.1-3.1h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1zM8 13h8v-2H8v2zm9-6h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1s-1.39 3.1-3.1 3.1h-4V17h4c2.76 0 5-2.24 5-5s-2.24-5-5-5z"/></svg>"#),
    ("icons/nt/info.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M11 7h2v2h-2zm0 4h2v6h-2zm1-9C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.41 0-8-3.59-8-8s3.59-8 8-8 8 3.59 8 8-3.59 8-8 8z"/></svg>"#),
    ("icons/nt/keyboard.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M20 5H4c-1.1 0-1.99.9-1.99 2L2 17c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm-9 3h2v2h-2V8zm0 3h2v2h-2v-2zM8 8h2v2H8V8zm0 3h2v2H8v-2zm-1 2H5v-2h2v2zm0-3H5V8h2v2zm9 7H8v-2h8v2zm0-4h-2v-2h2v2zm0-3h-2V8h2v2zm3 3h-2v-2h2v2zm0-3h-2V8h2v2z"/></svg>"#),
    ("icons/nt/check_circle.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"/></svg>"#),
    ("icons/nt/list.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 13h2v-2H3v2zm0 4h2v-2H3v2zm0-8h2V7H3v2zm4 4h14v-2H7v2zm0 4h14v-2H7v2zM7 7v2h14V7H7z"/></svg>"#),
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
