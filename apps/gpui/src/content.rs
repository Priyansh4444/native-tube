//! The page view's content: chip bar, the virtualized scroller, its blocks and the scrollbar.
//! Rendered inside `PageView` (a cached view) with `NtApp` as the context, so every listener
//! acts on the app directly.

use crate::app::{NtApp, now};
use crate::icons::icon;
use crate::pages::*;
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, HighlightStyle, InteractiveText, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, RenderImage, ScrollDelta, ScrollEvent,
    SharedString, StyledText, Window, canvas, div, img, px, relative,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;
use nt_core::{TOPICS, Video};
use std::ops::Range;
use std::sync::Arc;

type Cx<'a> = Context<'a, NtApp>;

/// What a block renderer needs, borrowed from the app (no per-frame copies of page data).
struct Ctx<'a> {
    page: &'a Page,
    images: &'a mut crate::images::ImageCache,
    prefs: &'a crate::prefs::Prefs,
    scale: f32,
    t: f64,
    /// The visible band (not the overscan): only here do spinners animate.
    vis: (f32, f32),
    win_w: f32,
}

impl Ctx<'_> {
    fn image(&mut self, url: Option<&str>, w: f32, cx: &mut Cx) -> Option<Arc<RenderImage>> {
        let url = url.filter(|u| !u.is_empty())?;
        self.images.get(
            url,
            w * self.scale,
            self.t,
            cx,
            |a| &mut a.images,
            NtApp::on_image,
        )
    }
}

fn picture(src: Option<Arc<RenderImage>>, w: f32, h: f32, radius: f32) -> Div {
    let base = div()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(px(radius))
        .bg(fill())
        .overflow_hidden();
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
    let base = div().size(px(size)).flex_none().rounded_full().bg(fill());
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

/// A pill button: primary (white) or fill.
fn pill(id: impl Into<gpui::ElementId>, primary: bool) -> gpui::Stateful<Div> {
    let (bg_, hov) = if primary {
        (text(), c(0xd9d9d9))
    } else {
        (fill(), fill_hover())
    };
    div()
        .id(id)
        .h(px(36.0))
        .px(px(16.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .rounded(px(18.0))
        .bg(bg_)
        .cursor_pointer()
        .hover(move |s| s.bg(hov))
        .active(|s| s.opacity(0.85))
}

fn badge(v: &Video) -> Option<Div> {
    let (label, bg_) = if v.is_live {
        ("LIVE".to_string(), red())
    } else {
        (v.duration.clone()?, badge_bg())
    };
    Some(
        div()
            .absolute()
            .bottom(px(8.0))
            .right(px(8.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(bg_)
            .child(txt(label, 12.0, 18.0, W500, text())),
    )
}

/// A thumbnail with its badge, the red "watched" bar, and press feedback.
fn thumb(ctx: &mut Ctx, v: &Video, w: f32, h: f32, radius: f32, cx: &mut Cx) -> Div {
    let src = ctx.image(Some(&v.thumbnail), w, cx);
    let watched = ctx.prefs.watched(&v.id);
    let mut p = picture(src, w, h, radius).relative();
    if let Some(b) = badge(v) {
        p = p.child(b);
    }
    if let Some(f) = watched {
        p = p.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(4.0))
                .bg(gpui::rgba(0x71717199))
                .child(div().h_full().w(relative(f.max(0.03))).bg(red())),
        );
    }
    p
}

/// The "⋮" button shown on hover; it opens the card menu at the pointer.
fn more_button(
    id: impl Into<gpui::ElementId>,
    group: &'static str,
    page: PageId,
    index: usize,
    cx: &mut Cx,
) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .top(px(-8.0))
        .right(px(-12.0))
        .size(px(40.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .opacity(0.0)
        .group_hover(group, |s| s.opacity(1.0))
        .hover(|s| s.bg(fill()))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
            cx.stop_propagation();
            let pos = ev.position();
            this.open_card_menu(page, index, f32::from(pos.x), f32::from(pos.y), cx);
        }))
        .child(icon("more_vert", 24.0, text()))
}

/// Ranges of `text` that match any word of `query` (case-insensitive), for bold terms.
fn term_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for word in query.split_whitespace().filter(|w| w.len() >= 2) {
        let w = word.to_lowercase();
        let mut start = 0;
        while let Some(i) = lower.get(start..).and_then(|s| s.find(&w)) {
            let a = start + i;
            let b = a + w.len();
            if text.is_char_boundary(a) && text.is_char_boundary(b) {
                out.push(a..b);
            }
            start = b;
        }
    }
    out.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in out {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

#[derive(Clone, Copy)]
enum LinkKind {
    Url,
    Tag,
    Time(f64),
}

/// URLs, #tags and timestamps in a description.
fn find_links(text: &str) -> Vec<(Range<usize>, LinkKind)> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let rest = text.get(i..).unwrap_or("");
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest
                .find(|c: char| c.is_whitespace())
                .map_or(text.len(), |e| i + e);
            out.push((i..end, LinkKind::Url));
            i = end;
            continue;
        }
        let prev_ok = i == 0
            || text
                .get(..i)
                .and_then(|s| s.chars().last())
                .is_some_and(|c| c.is_whitespace() || c == '(');
        if prev_ok && rest.starts_with('#') {
            let end = rest
                .char_indices()
                .skip(1)
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map_or(text.len(), |(e, _)| i + e);
            if end > i + 1 {
                out.push((i..end, LinkKind::Tag));
                i = end;
                continue;
            }
        }
        if prev_ok && rest.as_bytes().first().is_some_and(|b| b.is_ascii_digit()) {
            // h:mm:ss or m:ss
            let end = rest
                .char_indices()
                .find(|(_, c)| !(c.is_ascii_digit() || *c == ':'))
                .map_or(text.len(), |(e, _)| i + e);
            if let Some(token) = text.get(i..end) {
                let parts: Vec<&str> = token.split(':').collect();
                if (2..=3).contains(&parts.len())
                    && parts.iter().skip(1).all(|p| p.len() == 2)
                    && parts.iter().all(|p| !p.is_empty())
                {
                    let secs = parts
                        .iter()
                        .fold(0.0, |acc, p| acc * 60.0 + p.parse::<f64>().unwrap_or(0.0));
                    out.push((i..end, LinkKind::Time(secs)));
                    i = end;
                    continue;
                }
            }
        }
        i += rest.chars().next().map_or(1, |c| c.len_utf8());
    }
    out
}

fn spinner_block(ctx: &Ctx, b: &Block) -> AnyElement {
    let visible = b.y + b.h > ctx.vis.0 && b.y < ctx.vis.1;
    let el = div().size_full().flex().items_center().justify_center();
    // A spinner animates every frame; only in the visible band (review G6).
    if visible {
        el.child(Spinner::new().large().color(text2()))
            .into_any_element()
    } else {
        el.into_any_element()
    }
}

impl NtApp {
    pub fn render_page(&mut self, window: &mut Window, cx: &mut Cx) -> AnyElement {
        let geo = self.geo;
        let t = now();
        if self.chip_widths.is_empty() {
            self.chip_widths = TOPICS
                .iter()
                .map(|tp| measure_width(window, tp.title, 14.0, 500) + 24.0)
                .collect();
        }
        let NtApp {
            stack,
            images,
            prefs,
            offset,
            ..
        } = self;
        let Some(page) = stack.last() else {
            return div().into_any_element();
        };
        let home = page.is_home();
        let watch = page.is_watch();
        let top = if home { CHIPS_H } else { 0.0 };
        let vh = geo.viewport_h(home);
        let cw = geo.content_w(watch);
        let offset = *offset as f32;
        let topic = match page.kind {
            Kind::Home { topic } => Some(topic),
            _ => None,
        };
        // Build the viewport plus one viewport of overscan (half above, half below); request
        // images a full viewport beyond the screen without building those rows.
        let (blocks, prefetch, content_h) = match &page.layout {
            Some((_, lay)) => {
                let (a, b) = (offset - vh * 0.5, offset + vh * 1.5);
                let built: Vec<Block> = lay.visible(a, b).cloned().collect();
                let pre: Vec<Block> = lay
                    .visible(offset - vh, offset + 2.0 * vh)
                    .filter(|bl| bl.y + bl.h <= a || bl.y >= b)
                    .cloned()
                    .collect();
                (built, pre, lay.content_h)
            }
            None => (Vec::new(), Vec::new(), 0.0),
        };
        let mut ctx = Ctx {
            page,
            images,
            prefs,
            scale: geo.scale,
            t,
            vis: (offset, offset + vh),
            win_w: geo.win_w,
        };
        for b in &prefetch {
            prefetch_images(&mut ctx, b, cx);
        }
        let mut area = div()
            .id("scroller")
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(cw))
            .h(px(vh))
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                cx.stop_propagation();
                this.on_scroll_event(ev, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.cancel_fling();
                    window.focus(&this.focus, cx);
                }),
            );
        for b in &blocks {
            let el = block(&mut ctx, b, cx);
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
        let sb = scrollbar(self, vh, content_h, top, t, cx);
        area = area.children(sb);
        let mut root = div().relative().size_full().bg(bg()).child(area);
        if let Some(topic) = topic {
            root = root.child(self.chips_bar(topic, cw, geo.pad(), cx));
        }
        root.into_any_element()
    }

    fn chips_bar(&mut self, selected: usize, cw: f32, pad: f32, cx: &mut Cx) -> AnyElement {
        let total: f32 = self.chip_widths.iter().sum::<f32>()
            + 12.0 * (self.chip_widths.len().saturating_sub(1)) as f32;
        let avail = cw - 2.0 * pad;
        self.chips_max = (total - avail).max(0.0);
        self.chips_x = self.chips_x.clamp(0.0, self.chips_max);
        let x = self.chips_x;
        let mut row = div()
            .absolute()
            .top(px(12.0))
            .left(px(pad - x))
            .flex()
            .gap(px(12.0));
        for (i, tp) in TOPICS.iter().enumerate() {
            let on = i == selected;
            row = row.child(
                div()
                    .id(("chip", i))
                    .flex_none()
                    .h(px(32.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .rounded(px(8.0))
                    .bg(if on { text() } else { fill() })
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|s| s.bg(fill_hover())))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_topic(i, cx)))
                    .child(txt(
                        tp.title,
                        14.0,
                        20.0,
                        W500,
                        if on { on_light() } else { text() },
                    )),
            );
        }
        let arrow = |id: &'static str, name: &'static str, left: bool| {
            let fade = gpui::linear_gradient(
                if left { 90.0 } else { 270.0 },
                gpui::linear_color_stop(bg(), 0.5),
                gpui::linear_color_stop(bg().opacity(0.0), 1.0),
            );
            div()
                .absolute()
                .top_0()
                .h(px(CHIPS_H))
                .w(px(90.0))
                .when(left, |d| d.left_0().pl(px(pad - 8.0)))
                .when(!left, |d| {
                    d.right_0().pr(px(pad - 8.0)).flex().justify_end()
                })
                .bg(fade)
                .flex()
                .items_center()
                .child(
                    div()
                        .id(id)
                        .size(px(40.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(fill()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.scroll_chips(if left { -300.0 } else { 300.0 }, cx)
                        }))
                        .child(icon(name, 24.0, text())),
                )
        };
        div()
            .id("chips")
            .absolute()
            .left_0()
            .top_0()
            .w(px(cw))
            .h(px(CHIPS_H))
            .bg(bg())
            .overflow_hidden()
            // Horizontal wheel scrolls the chips; vertical wheel falls through to the page.
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                let dx = match ev.delta {
                    ScrollDelta::Lines(d) => -d.x * 40.0,
                    ScrollDelta::Pixels(d) => -f32::from(d.x),
                };
                if dx != 0.0 && this.chips_max > 0.0 {
                    cx.stop_propagation();
                    this.scroll_chips(dx, cx);
                }
            }))
            .child(row)
            .when(x > 0.5, |d| {
                d.child(arrow("chips-left", "chevron_left", true))
            })
            .when(x < self.chips_max - 0.5, |d| {
                d.child(arrow("chips-right", "chevron_right", false))
            })
            .into_any_element()
    }
}

fn measure_width(window: &mut Window, text_: &str, size: f32, weight: u16) -> f32 {
    let mut font = window.text_style().font();
    font.weight = FontWeight(weight as f32);
    let run = gpui::TextRun {
        len: text_.len(),
        font,
        color: text(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text_.to_string().into(), px(size), &[run], None);
    f32::from(line.width).ceil()
}

fn prefetch_images(ctx: &mut Ctx, b: &Block, cx: &mut Cx) {
    let page = ctx.page;
    match &b.kind {
        BlockKind::GridRow {
            first,
            count,
            card_w,
            avatar,
        } => {
            for v in page
                .feed
                .items
                .get(*first..first + count)
                .unwrap_or_default()
            {
                ctx.image(Some(&v.thumbnail), *card_w, cx);
                if *avatar {
                    ctx.image(v.channel_avatar.as_deref(), AVATAR, cx);
                }
            }
        }
        BlockKind::ListRow(i) => {
            if let Some(v) = page.feed.items.get(*i) {
                ctx.image(Some(&v.thumbnail), list_thumb_w(b.w), cx);
                ctx.image(v.channel_avatar.as_deref(), 24.0, cx);
            }
        }
        BlockKind::CompactRow(i) => {
            if let Some(v) = page.feed.items.get(*i) {
                ctx.image(Some(&v.thumbnail), COMPACT_THUMB_W, cx);
            }
        }
        BlockKind::Comment { i, .. } => {
            let url = page
                .watch()
                .and_then(|w| w.comments.get(*i))
                .and_then(|c| c.author_avatar.as_deref());
            ctx.image(url, 40.0, cx);
        }
        _ => {}
    }
}

fn scrollbar(
    app: &NtApp,
    vh: f32,
    content_h: f32,
    top: f32,
    t: f64,
    cx: &mut Cx,
) -> Option<AnyElement> {
    if content_h <= vh + 1.0 || !app.scrollbar_visible(t) {
        return None;
    }
    let max = (content_h - vh).max(1.0);
    let thumb = (vh * vh / content_h).max(32.0);
    let y = (app.offset as f32 / max).clamp(0.0, 1.0) * (vh - thumb);
    let dragging = app.sb.drag.is_some();
    let me = cx.entity().downgrade();
    // While dragging, follow the pointer anywhere in the window.
    let follow = dragging.then(|| {
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let m1 = me.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, _, window, cx| {
                    let y = f32::from(ev.position.y);
                    let _ = m1.update(cx, |app, cx| app.scroll_drag_to(y, window, cx));
                });
                let m2 = me.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    let _ = m2.update(cx, |app, cx| {
                        app.sb.drag = None;
                        app.content_changed(cx);
                    });
                });
            },
        )
        .absolute()
        .size_0()
    });
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
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let my = f32::from(ev.position.y) - TOP_BAR_H - top;
                    let t = now();
                    if my >= y && my <= y + thumb {
                        this.sb.drag = Some((f32::from(ev.position.y), this.offset));
                    } else if let Some(p) = this.top_mut() {
                        // Track click: a page toward the pointer, as Chrome does.
                        let step = p.scroll.page_step(vh as f64);
                        p.scroll.scroll_by(if my < y { -step } else { step }, t);
                    }
                    this.sb.last_activity = t;
                    this.kick(window, cx);
                    this.content_changed(cx);
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
            .children(follow)
            .into_any_element(),
    )
}

// ---- blocks ------------------------------------------------------------------------------

fn block(ctx: &mut Ctx, b: &Block, cx: &mut Cx) -> AnyElement {
    let page_id = ctx.page.id;
    match &b.kind {
        BlockKind::GridRow {
            first,
            count,
            card_w,
            avatar,
        } => {
            let mut row = div().flex().gap(px(COL_GAP));
            for k in 0..*count {
                let i = first + k;
                if ctx.page.feed.items.get(i).is_some() {
                    row = row.child(card(ctx, i, *card_w, *avatar, cx));
                }
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
                            div()
                                .w(px(*card_w))
                                .h(px(card_w * 9.0 / 16.0))
                                .rounded(px(12.0))
                                .bg(fill()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(12.0))
                                .child(div().size(px(36.0)).rounded_full().bg(fill()))
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .h(px(18.0))
                                                .w(relative(0.9))
                                                .rounded(px(4.0))
                                                .bg(fill()),
                                        )
                                        .child(
                                            div()
                                                .h(px(18.0))
                                                .w(relative(0.6))
                                                .rounded(px(4.0))
                                                .bg(fill()),
                                        ),
                                ),
                        ),
                );
            }
            row.into_any_element()
        }
        BlockKind::SkeletonList => {
            let tw = list_thumb_w(b.w);
            div()
                .flex()
                .gap(px(16.0))
                .child(
                    div()
                        .w(px(tw))
                        .h(px(tw * 9.0 / 16.0))
                        .rounded(px(12.0))
                        .bg(fill()),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .h(px(22.0))
                                .w(relative(0.8))
                                .rounded(px(4.0))
                                .bg(fill()),
                        )
                        .child(
                            div()
                                .h(px(14.0))
                                .w(relative(0.4))
                                .rounded(px(4.0))
                                .bg(fill()),
                        )
                        .child(
                            div()
                                .h(px(24.0))
                                .w(relative(0.3))
                                .rounded(px(4.0))
                                .bg(fill()),
                        ),
                )
                .into_any_element()
        }
        BlockKind::ListRow(i) => list_row(ctx, *i, b.w, b.h, cx),
        BlockKind::CompactRow(i) => compact_row(ctx, *i, cx),
        BlockKind::Spinner | BlockKind::CommentsSpinner => spinner_block(ctx, b),
        BlockKind::LoadError => {
            let p = match &ctx.page.status {
                Load::Failed(p) => p.clone(),
                _ => return div().into_any_element(),
            };
            error_state(&p, "retry-page", cx, |this, cx| this.retry_page(cx))
        }
        BlockKind::MoreError => {
            let Some(p) = ctx.page.feed.more_error.clone() else {
                return div().into_any_element();
            };
            error_state(&p, "retry-more", cx, |this, cx| this.retry_more(cx))
        }
        BlockKind::CommentsError => {
            let Some(CommentsState::Failed(p)) = ctx.page.watch().map(|w| w.comments_state.clone())
            else {
                return div().into_any_element();
            };
            error_state(&p, "retry-comments", cx, |this, cx| this.retry_comments(cx))
        }
        BlockKind::Empty(kind) => empty_state(kind, cx),
        BlockKind::PageTitle => {
            let history = matches!(ctx.page.kind, Kind::History);
            let has = !ctx.page.feed.items.is_empty();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_between()
                .child(txt(
                    if history {
                        "Watch history"
                    } else {
                        "Watch later"
                    },
                    20.0,
                    28.0,
                    W700,
                    text(),
                ))
                .when(history && has, |d| {
                    d.child(
                        pill("clear-history", false)
                            .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx)))
                            .child(icon("delete", 24.0, text()))
                            .child(txt("Clear all watch history", 14.0, 20.0, W500, text())),
                    )
                })
                .into_any_element()
        }
        BlockKind::ChannelBanner => {
            let url = match &ctx.page.kind {
                Kind::Channel { info: Some(i), .. } => i.banner.as_deref(),
                _ => None,
            };
            let src = ctx.image(url, b.w, cx);
            picture(src, b.w, b.h, 16.0).into_any_element()
        }
        BlockKind::ChannelHeader => channel_header(ctx, b.w, cx),
        BlockKind::ChannelTabs => div()
            .size_full()
            .border_b_1()
            .border_color(divider())
            .flex()
            .items_end()
            .child(
                div()
                    .h(px(48.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(text())
                    .child(txt("Videos", 16.0, 22.0, W500, text())),
            )
            .into_any_element(),
        BlockKind::SubsStrip => subs_strip(ctx, b.w, cx),
        BlockKind::Player => {
            let theatre = b.x == 0.0 && b.w >= ctx.win_w - 1.0;
            div()
                .size_full()
                .bg(black())
                .when(!theatre, |d| d.rounded(px(12.0)))
                .into_any_element()
        }
        BlockKind::WatchMeta { title_h } => watch_meta(ctx, b.w, *title_h, cx),
        BlockKind::WatchDesc { lines, shown } => watch_desc(ctx, *lines, *shown, cx),
        BlockKind::CommentsHeader => {
            let total = ctx.page.watch().and_then(|w| w.comments_total.clone());
            let title = match total {
                Some(n) => format!("{n} Comments"),
                None => "Comments".to_string(),
            };
            div()
                .size_full()
                .child(txt(title, 20.0, 28.0, W700, text()))
                .into_any_element()
        }
        BlockKind::Comment { i, lines, shown } => {
            comment(ctx, page_id, *i, b.w, *lines, *shown, cx)
        }
    }
}

fn error_state(
    p: &Problem,
    id: &'static str,
    cx: &mut Cx,
    retry: impl Fn(&mut NtApp, &mut Cx) + 'static,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .child(icon(
            if p.offline { "wifi_off" } else { "error" },
            64.0,
            text2(),
        ))
        .child(txt(p.title, 20.0, 28.0, W700, text()).mt(px(8.0)))
        .child(txt(p.detail, 14.0, 20.0, W400, text2()))
        .child(
            pill(id, false)
                .mt(px(12.0))
                .on_click(cx.listener(move |this, _, _, cx| retry(this, cx)))
                .child(txt("Retry", 14.0, 20.0, W500, text())),
        )
        .into_any_element()
}

fn empty_state(kind: &Empty, cx: &mut Cx) -> AnyElement {
    let (name, title, body, action): (&'static str, &str, &str, Option<&str>) = match kind {
        Empty::NoSubscriptions => (
            "subscriptions",
            "Don't miss new videos",
            "Subscribe to channels you like, and their newest videos show up here.",
            Some("Explore videos"),
        ),
        Empty::History => (
            "history",
            "This list has no videos",
            "Videos you watch show up here.",
            Some("Explore videos"),
        ),
        Empty::WatchLater => (
            "schedule",
            "No videos saved yet",
            "Save videos to watch later, and they show up here.",
            Some("Explore videos"),
        ),
        Empty::NoResults => (
            "search",
            "No results found",
            "Try different keywords or remove search filters.",
            None,
        ),
        Empty::NoVideos => (
            "video_library",
            "No videos here",
            "There is nothing to show yet.",
            None,
        ),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .child(icon(name, 120.0, text2()))
        .child(txt(title, 20.0, 28.0, W700, text()).mt(px(8.0)))
        .child(txt(body, 14.0, 20.0, W400, text2()).text_center())
        .when_some(action, |d, label| {
            d.child(
                pill("empty-action", true)
                    .mt(px(12.0))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.replace_root(Root::Home, window, cx)
                        }),
                    )
                    .child(txt(label, 14.0, 20.0, W500, on_light())),
            )
        })
        .into_any_element()
}

fn card(ctx: &mut Ctx, index: usize, w: f32, show_avatar: bool, cx: &mut Cx) -> AnyElement {
    let page_id = ctx.page.id;
    let Some(v) = ctx.page.feed.items.get(index) else {
        return div().into_any_element();
    };
    let th = w * 9.0 / 16.0;
    let pic = thumb(ctx, v, w, th, 12.0, cx);
    let av = if show_avatar {
        ctx.image(v.channel_avatar.as_deref(), AVATAR, cx)
    } else {
        None
    };
    let tw = card_text_w(w, show_avatar);
    let channel_id = v.channel_id.clone();
    let name = div()
        .id(("card-ch", index))
        .when_some(channel_id.clone(), |d, cid| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_channel(cid.clone(), window, cx);
                }))
        })
        .hover(|s| s.text_color(text()))
        .child(line(v.channel.clone(), 14.0, 20.0, W400, text2()));
    let av_el = div()
        .id(("card-av", index))
        .when_some(channel_id, |d, cid| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_channel(cid.clone(), window, cx);
                }))
        })
        .child(avatar(av, AVATAR));
    div()
        .id(("card", index))
        .group("card")
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_card_menu(
                    page_id,
                    index,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                    cx,
                );
            }),
        )
        .w(px(w))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(
            cx.listener(move |this, _, window, cx| this.open_feed_item(page_id, index, window, cx)),
        )
        .child(pic.id(("card-thumb", index)).active(|s| s.opacity(0.9)))
        .child(
            div()
                .mt(px(12.0))
                .flex()
                .gap(px(12.0))
                .when(show_avatar, |d| d.child(av_el))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(lines(v.title.clone(), 16.0, 22.0, W500, text(), 2).w(px(tw)))
                        .when(show_avatar, |d| d.child(name))
                        .child(line(v.meta_line(), 14.0, 20.0, W400, text2()))
                        .child(more_button(
                            ("card-more", index),
                            "card",
                            page_id,
                            index,
                            cx,
                        )),
                ),
        )
        .into_any_element()
}

fn list_row(ctx: &mut Ctx, index: usize, w: f32, h: f32, cx: &mut Cx) -> AnyElement {
    let page_id = ctx.page.id;
    let query = match &ctx.page.kind {
        Kind::Search { query } => Some(query.as_str()),
        _ => None,
    };
    let Some(v) = ctx.page.feed.items.get(index) else {
        return div().into_any_element();
    };
    let tw = list_thumb_w(w);
    let th = (tw * 9.0 / 16.0).round();
    let pic = thumb(ctx, v, tw, th, 12.0, cx);
    let av = ctx.image(v.channel_avatar.as_deref(), 24.0, cx);
    let channel_id = v.channel_id.clone();
    let ch = div()
        .id(("row-ch", index))
        .py(px(12.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .when_some(channel_id, |d, cid| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_channel(cid.clone(), window, cx);
                }))
        })
        .when(v.channel_avatar.is_some(), |d| d.child(avatar(av, 24.0)))
        .child(line(v.channel.clone(), 12.0, 18.0, W400, text2()));
    let snippet = v.snippet.as_ref().map(|s| {
        let ranges = query.map(|q| term_ranges(s, q)).unwrap_or_default();
        let styled = StyledText::new(SharedString::from(s.clone())).with_highlights(
            ranges.into_iter().map(|r| {
                (
                    r,
                    HighlightStyle {
                        font_weight: Some(W500),
                        color: Some(text()),
                        ..Default::default()
                    },
                )
            }),
        );
        div()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(text2())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(styled)
    });
    div()
        .id(("row", index))
        .group("row")
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_card_menu(
                    page_id,
                    index,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                    cx,
                );
            }),
        )
        .size_full()
        .h(px(h))
        .flex()
        .gap(px(16.0))
        .cursor_pointer()
        .on_click(
            cx.listener(move |this, _, window, cx| this.open_feed_item(page_id, index, window, cx)),
        )
        .child(pic.id(("row-thumb", index)).active(|s| s.opacity(0.9)))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(lines(v.title.clone(), 18.0, 26.0, W400, text(), 2).pr(px(40.0)))
                .child(line(v.meta_line(), 12.0, 18.0, W400, text2()))
                .child(ch)
                .children(snippet)
                .child(more_button(("row-more", index), "row", page_id, index, cx)),
        )
        .into_any_element()
}

fn compact_row(ctx: &mut Ctx, index: usize, cx: &mut Cx) -> AnyElement {
    let page_id = ctx.page.id;
    let Some(v) = ctx.page.feed.items.get(index) else {
        return div().into_any_element();
    };
    let pic = thumb(ctx, v, COMPACT_THUMB_W, COMPACT_THUMB_H, 8.0, cx);
    div()
        .id(("rel", index))
        .group("rel")
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_card_menu(
                    page_id,
                    index,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                    cx,
                );
            }),
        )
        .size_full()
        .flex()
        .gap(px(8.0))
        .cursor_pointer()
        .on_click(
            cx.listener(move |this, _, window, cx| this.open_feed_item(page_id, index, window, cx)),
        )
        .child(pic.id(("rel-thumb", index)).active(|s| s.opacity(0.9)))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(lines(v.title.clone(), 14.0, 20.0, W500, text(), 2).pr(px(24.0)))
                .child(line(v.channel.clone(), 12.0, 18.0, W400, text2()))
                .child(line(v.meta_line(), 12.0, 18.0, W400, text2()))
                .child(more_button(("rel-more", index), "rel", page_id, index, cx)),
        )
        .into_any_element()
}

fn subscribe_button(subscribed: bool, cx: &mut Cx) -> impl IntoElement {
    let b = pill("subscribe", !subscribed)
        .on_click(cx.listener(|this, _, _, cx| this.toggle_subscribe(cx)));
    if subscribed {
        b.child(icon("notifications", 24.0, text()))
            .child(txt("Subscribed", 14.0, 20.0, W500, text()))
            .child(icon("keyboard_arrow_down", 24.0, text()))
    } else {
        b.child(txt("Subscribe", 14.0, 20.0, W500, on_light()))
    }
}

fn channel_header(ctx: &mut Ctx, w: f32, cx: &mut Cx) -> AnyElement {
    let Kind::Channel {
        info: Some(info),
        subscribed,
        desc_expanded,
        ..
    } = &ctx.page.kind
    else {
        return div().into_any_element();
    };
    let small = ctx.win_w < 600.0;
    let size = if small { 72.0 } else { 160.0 };
    let av = ctx.image(info.avatar.as_deref(), size, cx);
    let mut meta: Vec<String> = Vec::new();
    meta.extend(info.handle.clone());
    meta.extend(info.subscribers.clone());
    meta.extend(info.video_count.clone());
    let tw = channel_text_w(w, size);
    let desc = info.description.trim().to_string();
    let expanded = *desc_expanded;
    let desc_el = (!desc.is_empty()).then(|| {
        let body = if expanded {
            txt(desc, 14.0, 20.0, W400, text2())
        } else {
            line(desc, 14.0, 20.0, W400, text2())
                .flex_shrink(1.0)
                .min_w(px(0.0))
        };
        div()
            .id("channel-desc")
            .w(px(tw))
            .when(!expanded, |d| d.flex().items_center())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(page) = this.top_mut() {
                    if let Kind::Channel { desc_expanded, .. } = &mut page.kind {
                        *desc_expanded = !*desc_expanded;
                    }
                    page.touch();
                }
                this.content_changed(cx);
            }))
            .child(body)
            .when(!expanded, |d| {
                d.child(
                    txt("...more", 14.0, 20.0, W500, text())
                        .flex_none()
                        .whitespace_nowrap(),
                )
            })
    });
    div()
        .size_full()
        .py(px(16.0))
        .flex()
        .gap(px(24.0))
        .child(avatar(av, size))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .justify_center()
                .child(line(
                    info.name.clone(),
                    if small { 24.0 } else { 36.0 },
                    if small { 32.0 } else { 44.0 },
                    W700,
                    text(),
                ))
                .child(line(meta.join(" • "), 14.0, 20.0, W400, text2()).mt(px(8.0)))
                .children(desc_el.map(|d| d.mt(px(8.0))))
                .child(
                    div()
                        .mt(px(12.0))
                        .flex()
                        .child(subscribe_button(*subscribed, cx)),
                ),
        )
        .into_any_element()
}

fn subs_strip(ctx: &mut Ctx, w: f32, cx: &mut Cx) -> AnyElement {
    let Kind::Subscriptions { channels } = &ctx.page.kind else {
        return div().into_any_element();
    };
    let per = 104.0;
    let fit = ((w / per).floor() as usize).max(1);
    let mut row = div()
        .w(px(w))
        .h_full()
        .flex()
        .gap(px(0.0))
        .overflow_hidden();
    for (i, ch) in channels.iter().take(fit).enumerate() {
        let av = ctx.image(ch.avatar.as_deref(), 64.0, cx);
        let cid = ch.id.clone();
        row = row.child(
            div()
                .id(("sub", i))
                .w(px(per))
                .flex_none()
                .py(px(8.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .rounded(px(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(fill()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_channel(cid.clone(), window, cx)
                }))
                .child(avatar(av, 64.0))
                .child(line(ch.name.clone(), 12.0, 18.0, W400, text()).max_w(px(per - 12.0))),
        );
    }
    row.into_any_element()
}

fn watch_meta(ctx: &mut Ctx, w: f32, title_h: f32, cx: &mut Cx) -> AnyElement {
    let Some(ws) = ctx.page.watch() else {
        return div().into_any_element();
    };
    let Some(info) = &ws.info else {
        return div().into_any_element();
    };
    // Some watch responses carry no channel name or avatar; the card had them.
    let name = if info.channel.name.is_empty() {
        ws.video.channel.clone()
    } else {
        info.channel.name.clone()
    };
    let av_url = info
        .channel
        .avatar
        .as_deref()
        .or(ws.video.channel_avatar.as_deref());
    let av = ctx.image(av_url, 40.0, cx);
    let cid = Some(info.channel.id.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| ws.video.channel_id.clone());
    let likes = info.likes.clone().unwrap_or_default();
    let compact = w < 760.0;
    let (subscribed, saved, vid) = (ws.subscribed, ws.saved, ws.video.id.clone());
    let cid2 = cid.clone();
    let owner = div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .min_w(px(0.0))
        .child(
            div()
                .id("watch-av")
                .when_some(cid.clone(), |d, cid| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_channel(cid.clone(), window, cx)
                        }))
                })
                .child(avatar(av, 40.0)),
        )
        .child(
            div()
                .id("watch-ch")
                .min_w(px(0.0))
                .flex_shrink(1.0)
                .flex()
                .flex_col()
                .when_some(cid2, |d, cid| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_channel(cid.clone(), window, cx)
                        }))
                })
                .child(line(name, 16.0, 22.0, W500, text()))
                .child(line(
                    info.channel.subscribers.clone().unwrap_or_default(),
                    12.0,
                    18.0,
                    W400,
                    text2(),
                )),
        )
        .child(div().ml(px(12.0)).child(subscribe_button(subscribed, cx)));
    // Like | dislike: counts only (the app never signs in, so they show, not act).
    let rating = div()
        .h(px(36.0))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(18.0))
        .bg(fill())
        .child(
            div()
                .h_full()
                .pl(px(12.0))
                .pr(px(12.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon("thumb_up", 24.0, text()))
                .when(!likes.is_empty(), |d| {
                    d.child(txt(likes, 14.0, 20.0, W500, text()))
                }),
        )
        .child(div().w(px(1.0)).h(px(24.0)).bg(divider()))
        .child(
            div()
                .h_full()
                .px(px(12.0))
                .flex()
                .items_center()
                .child(icon("thumb_down", 24.0, text())),
        );
    let share = pill("share", false)
        .on_click(cx.listener(move |this, _, _, cx| this.copy_link(&vid, cx)))
        .child(icon("share", 24.0, text()))
        .when(!compact, |d| {
            d.child(txt("Share", 14.0, 20.0, W500, text()))
        });
    let later = pill("watch-later", false)
        .on_click(cx.listener(|this, _, _, cx| this.toggle_watch_later(cx)))
        .child(icon(
            if saved { "bookmark_fill1" } else { "bookmark" },
            24.0,
            text(),
        ))
        .when(!compact, |d| {
            d.child(txt(
                if saved { "Saved" } else { "Watch later" },
                14.0,
                20.0,
                W500,
                text(),
            ))
        });
    div()
        .size_full()
        .pt(px(12.0))
        .flex()
        .flex_col()
        .child(
            txt(info.title.clone(), 20.0, 28.0, W700, text())
                .h(px(title_h))
                .w(px(bucket8(w))),
        )
        .child(
            div()
                .mt(px(12.0))
                .h(px(40.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(owner)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(px(8.0))
                        .child(rating)
                        .child(share)
                        .child(later),
                ),
        )
        .into_any_element()
}

fn watch_desc(ctx: &mut Ctx, lines_: usize, shown: usize, cx: &mut Cx) -> AnyElement {
    let Some(ws) = ctx.page.watch() else {
        return div().into_any_element();
    };
    let Some(info) = &ws.info else {
        return div().into_any_element();
    };
    let expanded = ws.desc_expanded;
    let head = [info.views.clone(), info.date.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ");
    let desc = info.description.trim().to_string();
    let links = find_links(&desc);
    let ranges: Vec<Range<usize>> = links.iter().map(|(r, _)| r.clone()).collect();
    let kinds: Vec<(LinkKind, String)> = links
        .iter()
        .map(|(r, k)| (*k, desc.get(r.clone()).unwrap_or("").to_string()))
        .collect();
    let styled = StyledText::new(SharedString::from(desc.clone())).with_highlights(
        ranges.iter().cloned().map(|r| {
            (
                r,
                HighlightStyle {
                    color: Some(link()),
                    ..Default::default()
                },
            )
        }),
    );
    let me = cx.entity().downgrade();
    let body = InteractiveText::new("desc-text", styled).on_click(ranges, move |ix, window, cx| {
        let _ = me.update(cx, |this, cx| {
            let Some((kind, s)) = kinds.get(ix) else {
                return;
            };
            match kind {
                LinkKind::Url => cx.open_url(s),
                LinkKind::Tag => this.submit_search(s.clone(), window, cx),
                LinkKind::Time(secs) => {
                    let at = *secs;
                    this.player.update(cx, |p, cx| {
                        let d = p.state.duration;
                        if d > 0.0 {
                            p.seek_fraction(at / d, cx);
                        }
                    });
                }
            }
        });
    });
    let more = (!expanded && lines_ > DESC_CLAMP).then(|| {
        div()
            .absolute()
            .right_0()
            .bottom_0()
            .pl(px(24.0))
            .bg(gpui::linear_gradient(
                90.0,
                gpui::linear_color_stop(c(0x272727).opacity(0.0), 0.0),
                gpui::linear_color_stop(c(0x272727), 0.25),
            ))
            .child(txt("...more", 14.0, 20.0, W500, text()))
    });
    div()
        .id("desc")
        .size_full()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(fill())
        .flex()
        .flex_col()
        .when(!expanded, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(fill_hover()))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_desc(cx)))
        })
        .child(line(head, 14.0, 20.0, W500, text()))
        .when(shown > 0, |d| {
            d.child(
                div()
                    .relative()
                    .text_size(px(14.0))
                    .line_height(px(BODY_LH))
                    .text_color(text())
                    .h(px(shown as f32 * BODY_LH))
                    .overflow_hidden()
                    .when(!expanded, |d| d.line_clamp(DESC_CLAMP))
                    .child(body)
                    .children(more),
            )
        })
        .when(expanded && lines_ > DESC_CLAMP, |d| {
            d.child(
                div()
                    .id("desc-less")
                    .mt(px(8.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_desc(cx)))
                    .child(txt("Show less", 14.0, 20.0, W500, text())),
            )
        })
        .into_any_element()
}

fn comment(
    ctx: &mut Ctx,
    page_id: PageId,
    i: usize,
    w: f32,
    lines_: usize,
    shown: usize,
    cx: &mut Cx,
) -> AnyElement {
    let url = ctx
        .page
        .watch()
        .and_then(|w| w.comments.get(i))
        .and_then(|c| c.author_avatar.clone());
    let av = ctx.image(url.as_deref(), 40.0, cx);
    let Some(cm) = ctx.page.watch().and_then(|w| w.comments.get(i)) else {
        return div().into_any_element();
    };
    let expanded = shown >= lines_;
    let clamped = lines_ > COMMENT_CLAMP;
    let tw = comment_text_w(w);
    let actions = div()
        .h(px(32.0))
        .mt(px(4.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(icon("thumb_up", 20.0, text()))
        .when_some(cm.likes.clone().filter(|l| l != "0"), |d, l| {
            d.child(txt(l, 12.0, 18.0, W400, text2()))
        })
        .child(icon("thumb_down", 20.0, text()).ml(px(12.0)));
    let _ = page_id;
    div()
        .w(px(w))
        .flex()
        .gap(px(16.0))
        .child(avatar(av, 40.0))
        .child(
            div()
                .w(px(tw))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(4.0))
                        .child(line(cm.author.clone(), 13.0, 18.0, W500, text()))
                        .when_some(cm.published.clone(), |d, p| {
                            d.child(txt(p, 12.0, 18.0, W400, text2()).whitespace_nowrap())
                        }),
                )
                .child(
                    div()
                        .mt(px(2.0))
                        .h(px(shown.max(1) as f32 * BODY_LH))
                        .overflow_hidden()
                        .child(
                            txt(cm.text.trim_end().to_string(), 14.0, BODY_LH, W400, text())
                                .when(!expanded, |d| d.line_clamp(COMMENT_CLAMP).text_ellipsis()),
                        ),
                )
                .when(clamped, |d| {
                    d.child(
                        div()
                            .id(("read-more", i))
                            .mt(px(4.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_comment(i, cx)))
                            .child(txt(
                                if expanded { "Show less" } else { "Read more" },
                                14.0,
                                20.0,
                                W500,
                                text2(),
                            )),
                    )
                })
                .child(actions),
        )
        .into_any_element()
}

impl NtApp {
    pub fn open_feed_item(&mut self, page: PageId, index: usize, window: &mut Window, cx: &mut Cx) {
        let v = self
            .stack
            .iter()
            .find(|p| p.id == page)
            .and_then(|p| p.feed.items.get(index))
            .cloned();
        if let Some(v) = v {
            self.open_video(v, window, cx);
        }
    }

    pub fn toggle_desc(&mut self, cx: &mut Cx) {
        if let Some(page) = self.top_mut() {
            if let Some(w) = page.watch_mut() {
                w.desc_expanded = !w.desc_expanded;
            }
            page.touch();
        }
        self.content_changed(cx);
    }

    pub fn toggle_comment(&mut self, i: usize, cx: &mut Cx) {
        if let Some(page) = self.top_mut() {
            if let Some(w) = page.watch_mut()
                && !w.expanded_comments.remove(&i)
            {
                w.expanded_comments.insert(i);
            }
            page.touch();
        }
        self.content_changed(cx);
    }
}
