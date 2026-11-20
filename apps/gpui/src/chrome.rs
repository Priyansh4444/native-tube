//! Window-wide chrome drawn by the root view: the full guide (pushed or as a drawer) and the
//! `?` keyboard-shortcut sheet.

use crate::app::{NtApp, Overlay, now};
use crate::icons::icon;
use crate::pages::*;
use crate::probe::{self, Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, ObjectFit, div, img, px};
use nt_core::library;

type Cx<'a> = Context<'a, NtApp>;

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
        "Watch later",
    ),
];

/// YouTube's player and page shortcuts, as its own `?` sheet groups them.
const SHORTCUTS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Playback",
        &[
            ("Toggle play/pause", "k / Space"),
            ("Rewind 10 seconds", "j"),
            ("Fast forward 10 seconds", "l"),
            ("Seek backward 5 seconds", "Left arrow"),
            ("Seek forward 5 seconds", "Right arrow"),
            ("Previous / next frame (paused)", ", / ."),
            ("Decrease / increase speed", "< / >"),
            ("Seek to 0%–90%", "0–9"),
            ("Seek to start / end", "Home / End"),
            ("Next video", "Shift+N"),
        ],
    ),
    (
        "General",
        &[
            ("Toggle full screen", "f"),
            ("Toggle theater mode", "t"),
            ("Toggle mute", "m"),
            ("Volume up / down", "Up / Down arrow"),
            ("Subtitles and closed captions", "c"),
            ("Exit full screen, close a menu", "Esc"),
        ],
    ),
    (
        "Navigation",
        &[
            ("Go to search box", "/"),
            ("Back / forward", "Alt+Left / Alt+Right"),
            ("Move focus", "Tab / Shift+Tab"),
            ("Show keyboard shortcuts", "?"),
        ],
    ),
];

impl NtApp {
    /// ☰: on wide non-Watch pages the guide pushes the page and the choice is remembered;
    /// elsewhere it opens as a drawer over the page.
    pub fn toggle_guide(&mut self, cx: &mut Cx) {
        let watch = self.top().is_some_and(|p| p.is_watch());
        if self.overlay == Overlay::Guide {
            self.set_overlay(Overlay::None, cx);
        } else if !watch && self.geo.win_w >= GUIDE_PUSH_MIN {
            self.prefs.guide = !self.prefs.guide;
            self.prefs.save();
            self.content_changed(cx);
        } else {
            self.set_overlay(Overlay::Guide, cx);
        }
    }

    pub fn set_overlay(&mut self, o: Overlay, cx: &mut Cx) {
        if self.overlay != o {
            self.overlay = o;
            self.menu = None;
            self.clear_hover(cx);
            cx.notify();
        }
    }

    /// The root page on screen, for the guide's selected entry.
    fn guide_selected(&self) -> Option<Root> {
        if self.stack.len() == 1 {
            self.top().and_then(|p| p.root())
        } else {
            None
        }
    }

    pub fn guide_list(&mut self, cx: &mut Cx) -> AnyElement {
        let selected = self.guide_selected();
        let layer = if self.overlay == Overlay::Guide {
            Layer::Overlay
        } else {
            probe::begin(cx, &[Layer::Rail]);
            Layer::Rail
        };
        let entry = |i: usize, cx: &mut Cx| {
            let (root, outline, filled, label) = ENTRIES[i];
            let on = selected == Some(root);
            div()
                .id(("guide", i))
                .children(target(cx, layer, "rail", label))
                .h(px(40.0))
                .px(px(12.0))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(24.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .when(on, |d| d.bg(fill()))
                .hover(|s| s.bg(fill_hover()))
                .tab_index(0)
                .focus_visible(focus_ring)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.overlay = Overlay::None;
                    this.replace_root(root, window, cx);
                }))
                .child(icon(if on { filled } else { outline }, 24.0, text()))
                .child(txt(label, 14.0, 20.0, if on { W500 } else { W400 }, text()))
        };
        let divider_ = || div().my(px(12.0)).h(px(1.0)).flex_none().bg(divider());
        let mut col = div()
            .id("guide-list")
            .size_full()
            .px(px(12.0))
            .py(px(12.0))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .tab_group()
            .tab_index(2)
            .tab_stop(false)
            .child(entry(0, cx))
            .child(entry(1, cx))
            .child(divider_())
            .child(
                div()
                    .h(px(40.0))
                    .px(px(12.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt("You", 16.0, 22.0, W500, text()))
                    .child(icon("chevron_right", 20.0, text())),
            )
            .child(entry(2, cx))
            .child(entry(3, cx));
        col = col.child(
            div()
                .id("queue-entry")
                .h(px(40.0))
                .px(px(12.0))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(24.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(|s| s.bg(fill()))
                .tab_index(0)
                .focus_visible(focus_ring)
                .children(target(cx, layer, "queue", "Queue"))
                .child(icon("list", 24.0, text()))
                .child(txt("Queue", 14.0, 20.0, W400, text()))
                .on_click(cx.listener(|a, _, _, cx| a.open_queue(cx))),
        );
        let subs = library().subscriptions();
        if !subs.is_empty() {
            col = col.child(divider_()).child(
                div()
                    .h(px(40.0))
                    .px(px(12.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(txt("Subscriptions", 16.0, 22.0, W500, text())),
            );
            let t = now();
            let scale = self.geo.scale;
            for (i, ch) in subs.iter().take(12).enumerate() {
                let av = ch.avatar.as_deref().and_then(|u| {
                    self.images
                        .get(u, 24.0 * scale, t, cx, |a| &mut a.images, NtApp::on_image)
                });
                let cid = ch.id.clone();
                col = col.child(
                    div()
                        .id(("guide-sub", i))
                        .children(target(cx, layer, "rail", ch.name.as_str()))
                        .h(px(40.0))
                        .px(px(12.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(24.0))
                        .rounded(px(10.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(fill_hover()))
                        .tab_index(0)
                        .focus_visible(focus_ring)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.overlay = Overlay::None;
                            this.open_channel(cid.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .size(px(24.0))
                                .flex_none()
                                .rounded_full()
                                .bg(fill())
                                .overflow_hidden()
                                .when_some(av, |d, a| {
                                    d.child(
                                        img(a)
                                            .size_full()
                                            .rounded_full()
                                            .object_fit(ObjectFit::Cover),
                                    )
                                }),
                        )
                        .child(line(ch.name.clone(), 14.0, 20.0, W400, text()).flex_1()),
                );
            }
        }
        col.into_any_element()
    }

    fn about_panel(&self, cx: &mut Cx) -> AnyElement {
        let Some((status, info)) = &self.about else {
            return div().into_any_element();
        };
        let w = (self.geo.win_w - 32.0).min(620.0);
        div()
            .id("channel-about")
            .absolute()
            .top(px(72.0))
            .left(px((self.geo.win_w - w) / 2.0))
            .w(px(w))
            .max_h(px(self.geo.win_h - 104.0))
            .overflow_y_scroll()
            .rounded(px(12.0))
            .bg(surface())
            .p(px(24.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(12.0))
                    .child(txt("About", 20.0, 28.0, W700, text()))
                    .child(
                        div()
                            .id("about-close")
                            .size(px(36.0))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .tab_index(0)
                            .focus_visible(focus_ring)
                            .children(target(cx, Layer::Overlay, "close", "Close About"))
                            .on_click(cx.listener(|a, _, _, cx| a.set_overlay(Overlay::None, cx)))
                            .child(icon("close", 24.0, text())),
                    ),
            )
            .child(txt(info.description.clone(), 14.0, 20.0, W400, text()))
            .children(
                info.links
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.url.starts_with("https://") || l.url.starts_with("http://"))
                    .map(|(i, l)| {
                        let url = l.url.clone();
                        div()
                            .id(("about-link", i))
                            .mt(px(12.0))
                            .tab_index(0)
                            .focus_visible(focus_ring)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .children(target(cx, Layer::Overlay, "link", l.label.as_str()))
                            .child(line(l.label.clone(), 14.0, 20.0, W500, link()))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    }),
            )
            .when(*status == Load::Loading, |d| {
                d.child(txt("Loading…", 14.0, 20.0, W400, text2()).mt(px(12.0)))
            })
            .when(matches!(status, Load::Failed(_)), |d| {
                d.child(
                    div()
                        .id("about-retry")
                        .mt(px(12.0))
                        .cursor_pointer()
                        .tab_index(0)
                        .focus_visible(focus_ring)
                        .children(target(cx, Layer::Overlay, "retry", "Retry"))
                        .child(txt(
                            "Couldn't load channel info · Retry",
                            14.0,
                            20.0,
                            W500,
                            link(),
                        ))
                        .on_click(cx.listener(|a, _, _, cx| a.open_channel_about(cx))),
                )
            })
            .into_any_element()
    }

    /// The drawer guide or the shortcut sheet, over everything, with a scrim that closes it.
    pub fn overlay_layer(&mut self, cx: &mut Cx) -> Option<AnyElement> {
        let body =
            match self.overlay {
                Overlay::None => return None,
                Overlay::Guide => div()
                    .id("guide-drawer")
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(GUIDE_W))
                    .bg(bg())
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(TOP_BAR_H))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .id("guide-close")
                                    .tab_index(0)
                                    .focus_visible(focus_ring)
                                    .children(target(cx, Layer::Overlay, "guide", "Close guide"))
                                    .size(px(40.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(fill()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_overlay(Overlay::None, cx)
                                    }))
                                    .child(icon("menu", 24.0, text())),
                            )
                            .child(txt("NativeTube", 20.0, 28.0, W700, text())),
                    )
                    .child(div().flex_1().min_h(px(0.0)).child(self.guide_list(cx)))
                    .into_any_element(),
                Overlay::Shortcuts => shortcut_sheet(self.geo, cx),
                Overlay::Playlist | Overlay::Queue => self.playlist_or_queue(cx),
                Overlay::About => self.about_panel(cx),
                Overlay::Filters => {
                    let Some(Page {
                        kind: Kind::Search { view, .. },
                        ..
                    }) = self.top()
                    else {
                        return None;
                    };
                    search_filters(&view.filters, self.geo, cx)
                }
            };
        let scrim = if self.overlay == Overlay::Guide {
            gpui::rgba(0x00000080)
        } else {
            gpui::rgba(0x000000b3)
        };
        Some(
            gpui::deferred(
                div()
                    .id("overlay-scrim")
                    .tab_group()
                    .tab_index(0)
                    .tab_stop(false)
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(scrim)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.set_overlay(Overlay::None, cx);
                        }),
                    )
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(body),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

fn search_filters(groups: &[nt_core::SearchFilterGroup], geo: Geo, cx: &mut Cx) -> AnyElement {
    let w = (geo.win_w - 32.0).min(720.0);
    div()
        .id("filter-sheet")
        .absolute()
        .top(px((geo.win_h * 0.15).max(16.0)))
        .left(px((geo.win_w - w) / 2.0))
        .w(px(w))
        .max_h(px(geo.win_h * 0.7))
        .overflow_y_scroll()
        .rounded(px(12.0))
        .bg(surface())
        .p(px(24.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(txt("Search filters", 20.0, 28.0, W700, text()))
                .child(
                    div()
                        .id("filter-close")
                        .size(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .tab_index(0)
                        .focus_visible(focus_ring)
                        .cursor_pointer()
                        .children(target(cx, Layer::Overlay, "close", "Close filters"))
                        .child(icon("close", 24.0, text()))
                        .on_click(cx.listener(|a, _, _, cx| a.set_overlay(Overlay::None, cx))),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(24.0))
                .mt(px(12.0))
                .children(groups.iter().enumerate().map(|(g, group)| {
                    div()
                        .min_w(px(96.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            txt(group.title.clone(), 14.0, 20.0, W700, text())
                                .pb(px(8.0))
                                .border_b_1()
                                .border_color(divider()),
                        )
                        .children(group.options.iter().enumerate().map(|(i, option)| {
                            div()
                                .id(gpui::SharedString::from(format!("filter-option-{g}-{i}")))
                                .px(px(4.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .child(txt(
                                    option.label.clone(),
                                    14.0,
                                    20.0,
                                    if option.selected { W700 } else { W400 },
                                    if option.selected { text() } else { text2() },
                                ))
                                .when_some(option.filter.clone(), |d, filter| {
                                    d.tab_index(0)
                                        .focus_visible(focus_ring)
                                        .children(target(
                                            cx,
                                            Layer::Overlay,
                                            "menu_item",
                                            option.label.as_str(),
                                        ))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(fill()))
                                        .on_click(cx.listener(move |a, _, window, cx| {
                                            a.filter_search(filter.clone(), window, cx)
                                        }))
                                })
                        }))
                })),
        )
        .into_any_element()
}

fn shortcut_sheet(geo: Geo, cx: &mut Cx) -> AnyElement {
    let w = (geo.win_w - 32.0).min(640.0);
    let h = (geo.win_h - 32.0).min(752.0);
    let mut sheet = div()
        .id("shortcuts")
        .absolute()
        .top(px((geo.win_h - h) / 2.0))
        .left(px((geo.win_w - w) / 2.0))
        .w(px(w))
        .max_h(px(h))
        .overflow_y_scroll()
        .p(px(24.0))
        .rounded(px(12.0))
        .bg(surface())
        .shadow_lg()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(8.0))
                .child(txt("Keyboard shortcuts", 20.0, 28.0, W700, text()))
                .child(
                    div()
                        .id("shortcuts-close")
                        .tab_index(0)
                        .focus_visible(focus_ring)
                        .children(target(cx, Layer::Overlay, "close", "Close shortcuts"))
                        .size(px(40.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(fill()))
                        .on_click(cx.listener(|this, _, _, cx| this.set_overlay(Overlay::None, cx)))
                        .child(icon("close", 24.0, text())),
                ),
        );
    for (title, rows) in SHORTCUTS {
        sheet = sheet.child(
            txt(title, 16.0, 22.0, W500, text())
                .mt(px(16.0))
                .mb(px(4.0)),
        );
        for (what, keys) in rows {
            sheet = sheet.child(
                div()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(txt(*what, 14.0, 20.0, W400, text()))
                    .child(shortcut_keys(keys)),
            );
        }
    }
    sheet.into_any_element()
}

fn shortcut_keys(keys: &str) -> AnyElement {
    let arrow = |radians| {
        icon("arrow_back", 20.0, text2())
            .with_transformation(gpui::Transformation::rotate(gpui::radians(radians)))
    };
    let pi = std::f32::consts::PI;
    let pair = |a, b| {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(arrow(a))
            .child(txt("/", 14.0, 20.0, W500, text2()))
            .child(arrow(b))
            .into_any_element()
    };
    match keys {
        "Left arrow" => arrow(0.0).into_any_element(),
        "Right arrow" => arrow(pi).into_any_element(),
        "Up / Down arrow" => pair(pi / 2.0, -pi / 2.0),
        "Alt+Left / Alt+Right" => div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(txt("Alt+", 14.0, 20.0, W500, text2()))
            .child(arrow(0.0))
            .child(txt("/  Alt+", 14.0, 20.0, W500, text2()))
            .child(arrow(pi))
            .into_any_element(),
        _ => txt(keys.to_owned(), 14.0, 20.0, W500, text2()).into_any_element(),
    }
}

/// The keyboard focus ring: 2 px #f1f1f1 at a 2 px offset (a dark gap, then the ring).
pub fn focus_ring(s: gpui::StyleRefinement) -> gpui::StyleRefinement {
    s.shadow(vec![
        gpui::BoxShadow {
            color: text(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(4.0),
            inset: false,
        },
        gpui::BoxShadow {
            color: bg(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
            inset: false,
        },
    ])
}
