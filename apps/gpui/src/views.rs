//! Element builders: top bar, navigation rail, the virtualized page scroller and its blocks.

use crate::app::{NtApp, now, search_key_handlers};
use crate::pages::*;
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, Hsla, MouseButton, MouseDownEvent, ObjectFit,
    RenderImage, ScrollEvent, SharedString, Stateful, deferred, div, img, px,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::skeleton::Skeleton;
use gpui_component::slider::Slider;
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Icon, Sizable as _};
use gpui_kit_assets::IconName as I;

fn icon(n: I) -> Icon {
    Icon::new(n)
}
use nt_core::{TOPICS, Video, format};
use std::sync::Arc;

type Cx<'a> = Context<'a, NtApp>;

fn image(
    app: &mut NtApp,
    url: Option<&str>,
    w: f32,
    t: f64,
    cx: &mut Cx,
) -> Option<Arc<RenderImage>> {
    let url = url?;
    let scale = app.geo.scale;
    app.images.get(url, w * scale, t, cx, |a| &mut a.images)
}

fn id(name: &'static str, i: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), i as u64)
}

/// A rounded picture with a raised placeholder until the pixels arrive.
fn picture(src: Option<Arc<RenderImage>>, w: f32, h: f32, radius: f32) -> Div {
    let base = div()
        .w(px(w))
        .h(px(h))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(raised());
    match src {
        Some(src) => base.child(
            img(src)
                .size_full()
                .rounded(px(radius))
                .object_fit(ObjectFit::Cover),
        ),
        None => base,
    }
}

fn avatar(src: Option<Arc<RenderImage>>, size: f32) -> Div {
    let base = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(raised());
    match src {
        Some(src) => base.child(
            img(src)
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        ),
        None => base,
    }
}

fn label(text: impl Into<SharedString>, size: f32, lh: f32, color: Hsla) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(lh))
        .text_color(color)
        .child(text.into())
}

fn one_line(text: impl Into<SharedString>, size: f32, lh: f32, color: Hsla) -> Div {
    label(text, size, lh, color)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
}

fn clamp_lines(
    text: impl Into<SharedString>,
    size: f32,
    lh: f32,
    color: Hsla,
    lines: usize,
) -> Div {
    label(text, size, lh, color)
        .line_clamp(lines)
        .text_ellipsis()
}

pub fn pill(
    name: &'static str,
    text: impl Into<SharedString>,
    bg_: Hsla,
    fg: Hsla,
    cx: &Cx,
) -> Button {
    let hover_c = if bg_ == raised() {
        hover()
    } else {
        bg_.opacity(0.85)
    };
    Button::new(name)
        .label(text)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(bg_)
                .foreground(fg)
                .hover(hover_c)
                .active(hover_c),
        )
        // The kit draws a custom variant's background at 20% strength; we want the token itself.
        .bg(bg_)
        .text_color(fg)
        .rounded(px(R_BUTTON))
}

fn duration_badge(v: &Video) -> Option<Div> {
    let (txt, bg_) = if v.is_live {
        ("LIVE".to_string(), accent())
    } else {
        (v.duration.clone()?, badge_bg())
    };
    Some(
        div()
            .absolute()
            .bottom(px(6.0))
            .right(px(6.0))
            .px(px(4.0))
            .rounded(px(4.0))
            .bg(bg_)
            .text_size(px(12.0))
            .line_height(px(18.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(text())
            .child(txt),
    )
}

fn thumb(app: &mut NtApp, v: &Video, w: f32, h: f32, radius: f32, t: f64, cx: &mut Cx) -> Div {
    let src = image(app, Some(&v.thumbnail), w, t, cx);
    let mut p = picture(src, w, h, radius).relative();
    if let Some(b) = duration_badge(v) {
        p = p.child(b);
    }
    p
}

fn channel_link(v: &Video, el: Stateful<Div>, cx: &mut Cx) -> Stateful<Div> {
    match v.channel_id.clone() {
        Some(cid) => el.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_channel(cid.clone(), cx);
            }),
        ),
        None => el,
    }
}

// ---- top bar -----------------------------------------------------------------------------

pub fn top_bar(app: &mut NtApp, cx: &mut Cx) -> AnyElement {
    let can_back = app.stack.len() > 1;
    let back = Button::new("back")
        .icon(icon(I::ArrowLeft))
        .ghost()
        .rounded(px(R_BUTTON))
        .disabled(!can_back)
        .on_click(cx.listener(|this, _, _, cx| this.back(cx)));

    let sugg = if app.sugg.open && !app.sugg.items.is_empty() {
        let mut list = div()
            .id("suggestions")
            .occlude()
            .absolute()
            .top(px(44.0))
            .left_0()
            .right(px(64.0))
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(c(0x212121))
            .border_1()
            .border_color(divider())
            .shadow_lg()
            .flex()
            .flex_col();
        for (i, s) in app.sugg.items.iter().enumerate() {
            let q = s.clone();
            let selected = app.sugg.selected == Some(i);
            list = list.child(
                div()
                    .id(id("sugg", i))
                    .h(px(36.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hover()))
                    .hover(|s| s.bg(hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.submit_search(q.clone(), window, cx);
                        }),
                    )
                    .child(div().text_color(text2()).child(icon(I::Search)))
                    .child(one_line(s.clone(), 16.0, 22.0, text()).font_weight(FontWeight::MEDIUM)),
            );
        }
        Some(deferred(list).with_priority(1))
    } else {
        None
    };

    let field = div()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .px(px(16.0))
        .bg(c(0x121212))
        .border_1()
        .border_color(divider())
        .rounded_l(px(20.0))
        .child(
            Input::new(&app.search)
                .appearance(false)
                .cleanable(true)
                .w_full(),
        );
    let go = Button::new("search-go")
        .icon(icon(I::Search))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(raised())
                .foreground(text())
                .hover(hover())
                .active(hover()),
        )
        .bg(raised())
        .w(px(64.0))
        .h(px(40.0))
        .rounded_r(px(20.0))
        .rounded_l(px(0.0))
        .on_click(cx.listener(|this, _, window, cx| {
            let q = this.search.read(cx).value().to_string();
            this.submit_search(q, window, cx);
        }));
    let search = search_key_handlers(div(), cx)
        .relative()
        .w_full()
        .max_w(px(640.0))
        .h(px(40.0))
        .flex()
        .child(field)
        .child(go)
        .children(sugg);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(TOP_BAR_H))
        .bg(bg())
        .flex()
        .items_center()
        .px(px(16.0))
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_shrink_0()
                .w(px(180.0))
                .child(back)
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .child("NativeTube"),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .flex()
                .justify_center()
                .child(search),
        )
        .child(div().flex_shrink(1.0).w(px(180.0)))
        .into_any_element()
}

// ---- navigation rail ---------------------------------------------------------------------

pub fn rail(app: &mut NtApp, cx: &mut Cx) -> AnyElement {
    let current = app.stack.first().and_then(|p| p.root());
    let entries = [
        (Root::Home, icon(I::House), "Home"),
        (Root::Subscriptions, icon(I::Users), "Subscriptions"),
        (Root::History, icon(I::RotateCcwClock), "History"),
        (Root::WatchLater, icon(I::Clock), "Watch Later"),
    ];
    let mut col = div()
        .absolute()
        .left_0()
        .top(px(TOP_BAR_H))
        .bottom_0()
        .w(px(RAIL_W))
        .bg(bg())
        .flex()
        .flex_col()
        .items_center()
        .pt(px(4.0))
        .gap(px(4.0));
    for (i, (root, icon, name)) in entries.into_iter().enumerate() {
        let active = current == Some(root) && app.stack.len() == 1;
        col = col.child(
            div()
                .id(id("rail", i))
                .w(px(64.0))
                .h(px(72.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .when(active, |d| d.bg(raised()))
                .hover(|s| s.bg(hover()))
                .on_click(cx.listener(move |this, _, _, cx| this.replace_root(root, cx)))
                .child(div().text_size(px(22.0)).child(icon))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(14.0))
                        .whitespace_nowrap()
                        .text_color(if active { text() } else { text2() })
                        .child(name),
                ),
        );
    }
    col.into_any_element()
}

// ---- the page scroller -------------------------------------------------------------------

pub fn content(app: &mut NtApp, t: f64, cx: &mut Cx) -> AnyElement {
    let geo = app.geo;
    let Some(page) = app.stack.last() else {
        return div().into_any_element();
    };
    let chips = page.has_chips();
    let top = geo.scroller_top(chips);
    let vh = geo.viewport_h(chips);
    let cw = geo.content_w();
    let offset = app.offset as f32;
    // Build the viewport plus one viewport of overscan (half above, half below); request
    // images a full viewport beyond the screen in both directions without building those rows.
    let (blocks, prefetch, content_h) = match &page.layout {
        Some((_, lay)) => {
            let (top_b, bot_b) = (offset - vh * 0.5, offset + vh * 1.5);
            let built: Vec<Block> = lay.visible(top_b, bot_b).cloned().collect();
            let pre: Vec<Block> = lay
                .visible(offset - vh, offset + 2.0 * vh)
                .filter(|b| b.y + b.h <= top_b || b.y >= bot_b)
                .cloned()
                .collect();
            (built, pre, lay.content_h)
        }
        None => (Vec::new(), Vec::new(), 0.0),
    };
    let topic = match page.kind {
        Kind::Home { topic } => Some(topic),
        _ => None,
    };
    for b in &prefetch {
        prefetch_images(app, b, t, cx);
    }

    let mut area = div()
        .id("scroller")
        .absolute()
        .left(px(RAIL_W))
        .top(px(top))
        .w(px(cw))
        .h(px(vh))
        .overflow_hidden()
        .track_focus(&app.focus)
        .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| this.on_scroll_event(ev, cx)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                if let Some(p) = this.top_mut() {
                    p.scroll.cancel_fling(now());
                }
                window.focus(&this.focus, cx);
            }),
        );
    for b in &blocks {
        let el = block(app, b, t, cx);
        area = area.child(
            div()
                .absolute()
                .left(px(b.x))
                .top(px(b.y - offset))
                .w(px(b.w))
                .h(px(b.h))
                .child(el),
        );
    }
    area = area.children(scrollbar(app, vh, content_h, t, cx));

    let mut root = div().absolute().left_0().top_0().size_full().child(area);
    if let Some(topic) = topic {
        root = root.child(chips_bar(topic, cw, cx));
    }
    root.into_any_element()
}

fn chips_bar(selected: usize, cw: f32, cx: &mut Cx) -> Div {
    let mut row = div()
        .absolute()
        .left(px(RAIL_W))
        .top(px(TOP_BAR_H))
        .w(px(cw))
        .h(px(CHIPS_H))
        .px(px(PAD))
        .bg(bg())
        .flex()
        .items_center()
        .gap(px(12.0))
        .overflow_hidden();
    for (i, topic) in TOPICS.iter().enumerate() {
        let on = i == selected;
        row = row.child(
            div()
                .id(id("chip", i))
                .flex_shrink_0()
                .h(px(32.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .rounded(px(R_CHIP))
                .bg(if on { chip_selected_bg() } else { raised() })
                .text_color(if on { chip_selected_fg() } else { text() })
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(!on, |d| d.hover(|s| s.bg(hover())))
                .on_click(cx.listener(move |this, _, _, cx| this.select_topic(i, cx)))
                .child(topic.title),
        );
    }
    row
}

fn scrollbar(app: &mut NtApp, vh: f32, content_h: f32, t: f64, cx: &mut Cx) -> Option<AnyElement> {
    if content_h <= vh + 1.0 || !app.scrollbar_visible(t) {
        return None;
    }
    let max = (content_h - vh).max(1.0);
    let thumb = (vh * vh / content_h).max(32.0);
    let y = (app.offset as f32 / max).clamp(0.0, 1.0) * (vh - thumb);
    let dragging = app.sb.drag.is_some();
    Some(
        div()
            .id("scrollbar")
            .absolute()
            .top_0()
            .right_0()
            .w(px(12.0))
            .h(px(vh))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let my = f32::from(ev.position.y);
                    let chips = this.top().is_some_and(|p| p.has_chips());
                    let top = this.geo.scroller_top(chips);
                    let off = this.offset;
                    let vh_ = this.geo.viewport_h(chips) as f64;
                    let t = now();
                    if my >= top + y && my <= top + y + thumb {
                        this.sb.drag = Some((my, off));
                    } else if let Some(p) = this.top_mut() {
                        // Track click: a page toward the pointer, as Chrome does.
                        let step = p.scroll.page_step(vh_);
                        p.scroll
                            .scroll_by(if my < top + y { -step } else { step }, t);
                    }
                    this.sb.last_activity = t;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(y))
                    .right(px(2.0))
                    .w(px(if dragging { 8.0 } else { 6.0 }))
                    .h(px(thumb))
                    .rounded(px(4.0))
                    .bg(if dragging { c(0x9a9a9a) } else { c(0x717171) }),
            )
            .into_any_element(),
    )
}

// ---- blocks ------------------------------------------------------------------------------

/// Ask for the images a block will need, without building it.
fn prefetch_images(app: &mut NtApp, b: &Block, t: f64, cx: &mut Cx) {
    let videos: Vec<(Video, f32, f32)> = {
        let Some(page) = app.stack.last() else { return };
        match &b.kind {
            BlockKind::GridRow {
                first,
                count,
                card_w,
            } => page.feed.items[*first..first + count]
                .iter()
                .map(|v| (v.clone(), *card_w, 36.0))
                .collect(),
            BlockKind::ListRow(i) => page
                .feed
                .items
                .get(*i)
                .map(|v| vec![(v.clone(), list_thumb_w(b.w), 24.0)])
                .unwrap_or_default(),
            BlockKind::CompactRow(i) => page
                .feed
                .items
                .get(*i)
                .map(|v| vec![(v.clone(), COMPACT_THUMB_W, 0.0)])
                .unwrap_or_default(),
            BlockKind::Comment(i) => {
                let url = page
                    .watch()
                    .and_then(|w| w.comments.get(*i))
                    .and_then(|c| c.author_avatar.clone());
                image(app, url.as_deref(), 40.0, t, cx);
                return;
            }
            _ => return,
        }
    };
    for (v, w, aw) in videos {
        image(app, Some(&v.thumbnail), w, t, cx);
        if aw > 0.0 {
            image(app, v.channel_avatar.as_deref(), aw, t, cx);
        }
    }
}

fn block(app: &mut NtApp, b: &Block, t: f64, cx: &mut Cx) -> AnyElement {
    match &b.kind {
        BlockKind::GridRow {
            first,
            count,
            card_w,
        } => {
            let items: Vec<Video> = app
                .stack
                .last()
                .map(|p| p.feed.items[*first..first + count].to_vec())
                .unwrap_or_default();
            let mut row = div().flex().gap(px(COL_GAP));
            for (k, v) in items.into_iter().enumerate() {
                row = row.child(card(app, first + k, v, *card_w, t, cx));
            }
            row.into_any_element()
        }
        BlockKind::SkeletonRow { cols, card_w } => {
            let mut row = div().flex().gap(px(COL_GAP));
            for _ in 0..*cols {
                row = row.child(
                    div()
                        .w(px(*card_w))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            Skeleton::new()
                                .w(px(*card_w))
                                .h(px(card_w * 9.0 / 16.0))
                                .rounded(px(R_THUMB)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(12.0))
                                .child(Skeleton::new().size(px(36.0)).rounded_full())
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(Skeleton::new().h(px(16.0)).w_full())
                                        .child(Skeleton::new().h(px(14.0)).w(px(card_w * 0.5))),
                                ),
                        ),
                );
            }
            row.into_any_element()
        }
        BlockKind::ListRow(i) => {
            let Some(v) = app.stack.last().and_then(|p| p.feed.items.get(*i)).cloned() else {
                return div().into_any_element();
            };
            list_row(app, *i, v, b.w, b.h, t, cx)
        }
        BlockKind::CompactRow(i) => {
            let Some(v) = app.stack.last().and_then(|p| p.feed.items.get(*i)).cloned() else {
                return div().into_any_element();
            };
            compact_row(app, *i, v, t, cx)
        }
        BlockKind::Spinner | BlockKind::CommentsSpinner => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(Spinner::new().large().color(text2()))
            .into_any_element(),
        BlockKind::LoadError | BlockKind::MoreError | BlockKind::CommentsError => {
            let msg = app
                .stack
                .last()
                .map(|p| {
                    match (
                        &p.status,
                        &p.feed.more_error,
                        p.watch().map(|w| &w.comments_state),
                    ) {
                        (Load::Failed(e), _, _) => e.clone(),
                        (_, Some(e), _) => e.clone(),
                        (_, _, Some(CommentsState::Failed(e))) => e.clone(),
                        _ => String::new(),
                    }
                })
                .unwrap_or_default();
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .child(
                    label("Something went wrong", 16.0, 22.0, text())
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    clamp_lines(msg, 14.0, 20.0, text2(), 2)
                        .max_w(px(560.0))
                        .text_center(),
                )
                .child(
                    pill("retry", "Retry", raised(), text(), cx)
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                )
                .into_any_element()
        }
        BlockKind::Empty(msg) => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(label(*msg, 16.0, 22.0, text2()))
            .into_any_element(),
        BlockKind::PageTitle => {
            let history = app
                .stack
                .last()
                .is_some_and(|p| matches!(p.kind, Kind::History));
            let has = app.stack.last().is_some_and(|p| !p.feed.items.is_empty());
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    label(
                        if history { "History" } else { "Watch Later" },
                        20.0,
                        28.0,
                        text(),
                    )
                    .font_weight(FontWeight::BOLD),
                )
                .when(history && has, |d| {
                    d.child(
                        pill("clear-history", "Clear history", raised(), text(), cx)
                            .icon(icon(I::Trash))
                            .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
                    )
                })
                .into_any_element()
        }
        BlockKind::ChannelBanner => {
            let url = app.stack.last().and_then(|p| match &p.kind {
                Kind::Channel { info: Some(i), .. } => i.banner.clone(),
                _ => None,
            });
            let src = image(app, url.as_deref(), b.w, t, cx);
            picture(src, b.w, b.h, R_THUMB).into_any_element()
        }
        BlockKind::ChannelHeader => channel_header(app, t, cx),
        BlockKind::SubsStrip => subs_strip(app, b.w, t, cx),
        BlockKind::Player => player(app, b, cx),
        BlockKind::WatchMeta { title_h } => watch_meta(app, *title_h, t, cx),
        BlockKind::WatchDesc { lines, shown } => watch_desc(app, *lines, *shown, cx),
        BlockKind::CommentsHeader => {
            let total = app
                .stack
                .last()
                .and_then(|p| p.watch())
                .and_then(|w| w.comments_total.clone());
            let title = match total {
                Some(n) => format!("{n} Comments"),
                None => "Comments".to_string(),
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .child(label(title, 20.0, 28.0, text()).font_weight(FontWeight::BOLD))
                .into_any_element()
        }
        BlockKind::Comment(i) => comment(app, *i, b.w, t, cx),
    }
}

fn card(app: &mut NtApp, index: usize, v: Video, w: f32, t: f64, cx: &mut Cx) -> AnyElement {
    let th = w * 9.0 / 16.0;
    let pic = thumb(app, &v, w, th, R_THUMB, t, cx);
    let av = image(app, v.channel_avatar.as_deref(), 36.0, t, cx);
    let has_avatar = v.channel_avatar.is_some();
    let open = v.clone();
    let name = channel_link(&v, div().id(id("card-ch", index)), cx).child(one_line(
        v.channel.clone(),
        14.0,
        20.0,
        text2(),
    ));
    let av_el = channel_link(&v, div().id(id("card-av", index)), cx).child(avatar(av, 36.0));
    div()
        .id(id("card", index))
        .w(px(w))
        .h(px(card_h(w)))
        .flex()
        .flex_col()
        .rounded(px(R_THUMB))
        .cursor_pointer()
        .hover(|s| s.bg(c(0x1f1f1f)))
        .on_click(cx.listener(move |this, _, _, cx| this.open_video(open.clone(), cx)))
        .child(pic)
        .child(
            div()
                .mt(px(12.0))
                .px(px(4.0))
                .flex()
                .gap(px(12.0))
                .when(has_avatar, |d| d.child(av_el))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            clamp_lines(v.title.clone(), 16.0, 22.0, text(), 2)
                                .h(px(44.0))
                                .mb(px(4.0))
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(name)
                        .child(one_line(v.meta_line(), 14.0, 20.0, text2())),
                ),
        )
        .into_any_element()
}

fn list_row(
    app: &mut NtApp,
    index: usize,
    v: Video,
    w: f32,
    h: f32,
    t: f64,
    cx: &mut Cx,
) -> AnyElement {
    let tw = list_thumb_w(w);
    let pic = thumb(app, &v, tw, h, R_THUMB, t, cx);
    let av = image(app, v.channel_avatar.as_deref(), 24.0, t, cx);
    let open = v.clone();
    let ch = channel_link(&v, div().id(id("row-ch", index)), cx)
        .flex()
        .items_center()
        .gap(px(8.0))
        .my(px(8.0))
        .when(v.channel_avatar.is_some(), |d| d.child(avatar(av, 24.0)))
        .child(one_line(v.channel.clone(), 12.0, 18.0, text2()));
    div()
        .id(id("row", index))
        .size_full()
        .flex()
        .gap(px(16.0))
        .rounded(px(R_THUMB))
        .cursor_pointer()
        .hover(|s| s.bg(c(0x1f1f1f)))
        .on_click(cx.listener(move |this, _, _, cx| this.open_video(open.clone(), cx)))
        .child(pic)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    clamp_lines(v.title.clone(), 18.0, 26.0, text(), 2)
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(one_line(v.meta_line(), 12.0, 18.0, text2()))
                .child(ch)
                .children(
                    v.snippet
                        .clone()
                        .map(|s| clamp_lines(s, 12.0, 18.0, text2(), 2)),
                ),
        )
        .into_any_element()
}

fn compact_row(app: &mut NtApp, index: usize, v: Video, t: f64, cx: &mut Cx) -> AnyElement {
    let pic = thumb(app, &v, COMPACT_THUMB_W, COMPACT_THUMB_H, 8.0, t, cx);
    let open = v.clone();
    div()
        .id(id("rel", index))
        .size_full()
        .flex()
        .gap(px(8.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(c(0x1f1f1f)))
        .on_click(cx.listener(move |this, _, _, cx| this.open_video(open.clone(), cx)))
        .child(pic)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    clamp_lines(v.title.clone(), 14.0, 20.0, text(), 2)
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(one_line(v.channel.clone(), 12.0, 18.0, text2()))
                .child(one_line(v.meta_line(), 12.0, 18.0, text2())),
        )
        .into_any_element()
}

fn channel_header(app: &mut NtApp, t: f64, cx: &mut Cx) -> AnyElement {
    let Some((info, subscribed)) = app.stack.last().and_then(|p| match &p.kind {
        Kind::Channel {
            info: Some(i),
            subscribed,
            ..
        } => Some((i.clone(), *subscribed)),
        _ => None,
    }) else {
        return div().into_any_element();
    };
    let av = image(app, info.avatar.as_deref(), 80.0, t, cx);
    let meta: Vec<String> = [
        info.handle.clone(),
        info.subscribers.clone(),
        info.video_count.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    div()
        .size_full()
        .flex()
        .items_center()
        .gap(px(24.0))
        .child(avatar(av, 80.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    one_line(info.name.clone(), 20.0, 28.0, text()).font_weight(FontWeight::BOLD),
                )
                .child(one_line(meta.join(" • "), 14.0, 20.0, text2()))
                .child(clamp_lines(
                    info.description.clone(),
                    14.0,
                    20.0,
                    text2(),
                    2,
                ))
                .child(
                    div()
                        .mt(px(8.0))
                        .flex()
                        .child(subscribe_button(subscribed, cx)),
                ),
        )
        .into_any_element()
}

fn subscribe_button(subscribed: bool, cx: &mut Cx) -> Button {
    let b = if subscribed {
        pill("subscribe", "Subscribed", raised(), text(), cx)
    } else {
        pill("subscribe", "Subscribe", accent(), c(0xffffff), cx)
    };
    b.on_click(cx.listener(|this, _, _, cx| this.toggle_subscribe(cx)))
}

fn subs_strip(app: &mut NtApp, w: f32, t: f64, cx: &mut Cx) -> AnyElement {
    let channels = app.stack.last().and_then(|p| match &p.kind {
        Kind::Subscriptions { channels } => Some(channels.clone()),
        _ => None,
    });
    let mut row = div()
        .w(px(w))
        .h_full()
        .flex()
        .gap(px(8.0))
        .overflow_hidden();
    for (i, ch) in channels.unwrap_or_default().into_iter().enumerate() {
        let av = image(app, ch.avatar.as_deref(), 64.0, t, cx);
        let cid = ch.id.clone();
        row = row.child(
            div()
                .id(id("sub", i))
                .w(px(96.0))
                .flex_shrink_0()
                .py(px(8.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .rounded(px(R_THUMB))
                .cursor_pointer()
                .hover(|s| s.bg(c(0x1f1f1f)))
                .on_click(cx.listener(move |this, _, _, cx| this.open_channel(cid.clone(), cx)))
                .child(avatar(av, 64.0))
                .child(one_line(ch.name.clone(), 12.0, 18.0, text()).max_w(px(88.0))),
        );
    }
    row.into_any_element()
}

// ---- watch page --------------------------------------------------------------------------

fn player(app: &mut NtApp, b: &Block, cx: &mut Cx) -> AnyElement {
    let scale = app.geo.scale;
    app.video
        .set_target((b.w * scale).round() as u32, (b.h * scale).round() as u32);
    let s = app.video.state.clone();
    let err = app.video.error.clone().or(match &s.status {
        nt_core::Status::Failed(e) => Some(e.clone()),
        _ => None,
    });
    let loading = err.is_none()
        && (matches!(s.status, nt_core::Status::Loading | nt_core::Status::Idle) || s.buffering);
    let paused = s.paused || s.status == nt_core::Status::Ended;
    let frac = app.seek_drag.unwrap_or(if s.duration > 0.0 {
        (s.position / s.duration) as f32
    } else {
        0.0
    });
    let buffered = if s.duration > 0.0 {
        ((s.position + s.buffered_ahead) / s.duration).min(1.0) as f32
    } else {
        0.0
    };
    // Seek bar geometry in window coordinates, for drags handled at the root.
    let bar_x = RAIL_W + b.x + 12.0;
    let bar_w = b.w - 24.0;
    app.seek_bar = Some((bar_x, bar_w));

    let mut surface = div()
        .id("player")
        .group("player")
        .relative()
        .size_full()
        .bg(c(0x000000))
        .rounded(px(R_THUMB))
        .overflow_hidden();
    if let Some(frame) = app.video.image.clone()
        && err.is_none()
    {
        surface = surface.child(
            img(frame)
                .size_full()
                .rounded(px(R_THUMB))
                .object_fit(ObjectFit::Contain),
        );
    }
    surface = surface.on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, _: &MouseDownEvent, _, cx| this.toggle_pause(cx)),
    );
    if loading {
        surface = surface.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().large().color(text())),
        );
    }
    if let Some(e) = err {
        surface = surface.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .p(px(24.0))
                .child(label("Playback failed", 16.0, 22.0, text()).font_weight(FontWeight::MEDIUM))
                .child(clamp_lines(e, 14.0, 20.0, text2(), 3).text_center()),
        );
    }

    let seek = div()
        .id("seek")
        .relative()
        .w_full()
        .h(px(16.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some((x0, w)) = this.seek_bar {
                    let f = ((f32::from(ev.position.x) - x0) / w.max(1.0)).clamp(0.0, 1.0);
                    this.seek_drag = Some(f);
                    this.seek_fraction(f);
                    cx.notify();
                }
            }),
        )
        .child(
            div()
                .relative()
                .w_full()
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(c(0xffffff).opacity(0.25))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(px(bar_w * buffered))
                        .rounded(px(2.0))
                        .bg(c(0xffffff).opacity(0.45)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(px(bar_w * frac))
                        .rounded(px(2.0))
                        .bg(accent()),
                ),
        )
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(px(bar_w * frac - 6.0))
                .size(px(12.0))
                .rounded_full()
                .bg(accent()),
        );

    let pos = app
        .seek_drag
        .map(|f| f as f64 * s.duration)
        .unwrap_or(s.position);
    let time = format!("{} / {}", format::clock(pos), format::clock(s.duration));
    let play_icon = if paused {
        icon(I::Play)
    } else {
        icon(I::Pause)
    };
    let mute_icon = if s.muted || s.volume <= 0.0 {
        icon(I::VolumeX)
    } else {
        icon(I::Volume2)
    };
    let controls = div()
        .id("controls")
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .px(px(12.0))
        .pb(px(8.0))
        .pt(px(24.0))
        .bg(gpui::linear_gradient(
            0.0,
            gpui::linear_color_stop(c(0x000000).opacity(0.75), 0.0),
            gpui::linear_color_stop(c(0x000000).opacity(0.0), 1.0),
        ))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .when(!paused, |d| {
            d.opacity(0.0).group_hover("player", |s| s.opacity(1.0))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(seek)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    Button::new("play")
                        .icon(play_icon)
                        .ghost()
                        .rounded(px(R_BUTTON))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
                )
                .child(
                    Button::new("mute")
                        .icon(mute_icon)
                        .ghost()
                        .rounded(px(R_BUTTON))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_mute();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .w(px(96.0))
                        .child(Slider::new(&app.volume).horizontal()),
                )
                .child(
                    div()
                        .ml(px(8.0))
                        .text_size(px(14.0))
                        .text_color(text())
                        .child(time),
                ),
        );
    surface.child(controls).into_any_element()
}

fn watch_meta(app: &mut NtApp, title_h: f32, t: f64, cx: &mut Cx) -> AnyElement {
    let Some((info, subscribed, saved, card_channel)) =
        app.stack.last().and_then(|p| p.watch()).and_then(|w| {
            w.info
                .clone()
                .map(|i| (i, w.subscribed, w.saved, w.video.channel.clone()))
        })
    else {
        return div().into_any_element();
    };
    // Some watch responses (multi-artist videos) carry no channel name; the card had one.
    let channel_name = if info.channel.name.is_empty() {
        card_channel
    } else {
        info.channel.name.clone()
    };
    let av = image(app, info.channel.avatar.as_deref(), 40.0, t, cx);
    let cid = info.channel.id.clone();
    let cid2 = cid.clone();
    let likes = info.likes.clone().unwrap_or_else(|| "Like".into());
    div()
        .size_full()
        .flex()
        .flex_col()
        .pt(px(12.0))
        .child(
            label(info.title.clone(), 20.0, TITLE_LH, text())
                .h(px(title_h))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            div()
                .mt(px(12.0))
                .h(px(48.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .id("watch-av")
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_channel(cid.clone(), cx)),
                        )
                        .child(avatar(av, 40.0)),
                )
                .child(
                    div()
                        .id("watch-ch")
                        .min_w(px(0.0))
                        .flex_shrink(1.0)
                        .flex()
                        .flex_col()
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_channel(cid2.clone(), cx)),
                        )
                        .child(
                            one_line(channel_name, 16.0, 22.0, text())
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(one_line(
                            info.channel.subscribers.clone().unwrap_or_default(),
                            12.0,
                            18.0,
                            text2(),
                        )),
                )
                .child(div().ml(px(12.0)).child(subscribe_button(subscribed, cx)))
                .child(div().flex_1())
                .child(
                    div()
                        .h(px(36.0))
                        .px(px(16.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(R_BUTTON))
                        .bg(raised())
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(I::ThumbsUp))
                        .child(likes),
                )
                .child(
                    pill(
                        "watch-later",
                        if saved { "Saved" } else { "Watch Later" },
                        raised(),
                        text(),
                        cx,
                    )
                    .icon(if saved {
                        icon(I::BookmarkCheck)
                    } else {
                        icon(I::Bookmark)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_watch_later(cx))),
                ),
        )
        .into_any_element()
}

fn watch_desc(app: &mut NtApp, lines: usize, shown: usize, cx: &mut Cx) -> AnyElement {
    let Some((info, expanded)) = app
        .stack
        .last()
        .and_then(|p| p.watch())
        .and_then(|w| w.info.clone().map(|i| (i, w.desc_expanded)))
    else {
        return div().into_any_element();
    };
    let head = [info.views.clone(), info.date.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ");
    let desc = info.description.trim_end().to_string();
    let mut body = div()
        .size_full()
        .p(px(12.0))
        .rounded(px(R_THUMB))
        .bg(raised())
        .flex()
        .flex_col()
        .gap(px(4.0));
    body = body.child(one_line(head, 14.0, 20.0, text()).font_weight(FontWeight::BOLD));
    if shown > 0 {
        let mut d = label(desc, 14.0, BODY_LH, text());
        if !expanded {
            d = d.line_clamp(DESC_CLAMP).text_ellipsis();
        }
        body = body.child(d.h(px(shown as f32 * BODY_LH)));
    }
    if lines > DESC_CLAMP {
        body = body.child(
            div()
                .id("desc-toggle")
                .text_size(px(14.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(page) = this.top_mut() {
                        if let Some(w) = page.watch_mut() {
                            w.desc_expanded = !w.desc_expanded;
                        }
                        page.touch();
                    }
                    cx.notify();
                }))
                .child(if expanded { "Show less" } else { "Show more" }),
        );
    }
    body.into_any_element()
}

fn comment(app: &mut NtApp, i: usize, w: f32, t: f64, cx: &mut Cx) -> AnyElement {
    let Some(cm) = app
        .stack
        .last()
        .and_then(|p| p.watch())
        .and_then(|w| w.comments.get(i))
        .cloned()
    else {
        return div().into_any_element();
    };
    let av = image(app, cm.author_avatar.as_deref(), 40.0, t, cx);
    let mut head = cm.author.clone();
    if let Some(p) = &cm.published {
        head.push_str(" • ");
        head.push_str(p);
    }
    let mut actions = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(12.0))
        .line_height(px(20.0))
        .text_color(text2());
    actions = actions
        .child(div().text_size(px(14.0)).child(icon(I::ThumbsUp)))
        .child(cm.likes.clone().unwrap_or_default());
    if let Some(r) = cm.reply_count.clone().filter(|r| r != "0") {
        actions = actions.child(
            div()
                .ml(px(12.0))
                .text_color(c(0x3ea6ff))
                .child(format!("{r} replies")),
        );
    }
    let _ = cx;
    div()
        .w(px(w))
        .flex()
        .gap(px(12.0))
        .child(avatar(av, 40.0))
        .child(
            div()
                .w(px(comment_text_w(w)))
                .flex()
                .flex_col()
                .child(one_line(head, 12.0, 18.0, text2()).mb(px(4.0)))
                .child(label(cm.text.trim_end().to_string(), 14.0, BODY_LH, text()))
                .child(actions.mt(px(6.0))),
        )
        .into_any_element()
}
