//! The navigation rail: Home, Subscriptions, History, Watch Later. A cached view; it re-renders
//! only when the selected entry changes.

use crate::icons::icon;
use crate::pages::Root;
use crate::probe::{self, Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{Context, EventEmitter, Window, div, px};

pub struct Rail {
    /// The entry shown as selected: only when that root page is what's on screen.
    pub selected: Option<Root>,
}

pub struct Navigate(pub Root);

impl EventEmitter<Navigate> for Rail {}

const ENTRIES: [(Root, &str, &str, &str); 4] = [
    (Root::Home, "home", "home_fill1", "Home"),
    (
        Root::Subscriptions,
        "subscriptions",
        "subscriptions_fill1",
        "Subscriptions",
    ),
    (Root::History, "history", "history_fill1", "History"),
    (
        Root::WatchLater,
        "schedule",
        "schedule_fill1",
        "Watch Later",
    ),
];

impl Render for Rail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        probe::begin(cx, &[Layer::Rail]);
        let mut col = div()
            .id("rail-list")
            .tab_group()
            .tab_index(2)
            .tab_stop(false)
            .size_full()
            .bg(bg())
            .flex()
            .flex_col()
            .items_center()
            .px(px(4.0));
        for (i, (root, outline, filled, label)) in ENTRIES.into_iter().enumerate() {
            let on = self.selected == Some(root);
            col = col.child(
                div()
                    .id(("rail", i))
                    .tab_index(0)
                    .focus_visible(crate::chrome::focus_ring)
                    .children(target(cx, Layer::Rail, "rail", label))
                    .w(px(64.0))
                    .h(px(74.0))
                    .pt(px(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(fill()))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(Navigate(root))))
                    .child(icon(if on { filled } else { outline }, 24.0, text()))
                    .child(
                        txt(label, 10.0, 14.0, W400, text())
                            .mt(px(6.0))
                            .whitespace_nowrap(),
                    ),
            );
        }
        col
    }
}
