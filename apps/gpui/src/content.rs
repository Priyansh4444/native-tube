//! The page view's content: chip bar, the virtualized scroller, its blocks and the scrollbar.
//! Rendered inside `PageView` (a cached view) with `NtApp` as the context, so every listener
//! acts on the app directly.

use crate::app::{NtApp, Overlay, now};
use crate::chrome::focus_ring;
use crate::icons::icon;
use crate::pages::*;
use crate::probe::{self, Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, HighlightStyle, InteractiveText, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, RenderImage, ScrollDelta, ScrollEvent,
    SharedString, StyledText, Window, canvas, div, img, px, relative,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;
use nt_core::{ChannelTab, Comment, PlaylistRef, TOPICS, Video};
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
    tab: bool,
    request_images: bool,
    preview: Option<(usize, gpui::Entity<crate::preview::HoverPreview>)>,
}

impl Ctx<'_> {
    fn image(&mut self, url: Option<&str>, w: f32, cx: &mut Cx) -> Option<Arc<RenderImage>> {
        let url = url.filter(|u| !u.is_empty())?;
        if !self.request_images {
            return self.images.ready(url);
        }
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
        .tab_index(0)
        .focus_visible(focus_ring)
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
    if v.upcoming.is_some() {
        return None;
    }
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
fn thumb(
    ctx: &mut Ctx,
    v: &Video,
    (w, h): (f32, f32),
    radius: f32,
    index: usize,
    cx: &mut Cx,
) -> Div {
    let src = ctx.image(Some(&v.thumbnail), w, cx);
    let watched = ctx.prefs.watched(&v.id);
    let mut p = picture(src, w, h, radius).relative();
    if let Some((_, preview)) = ctx.preview.as_ref().filter(|(i, _)| *i == index) {
        p = p.child(
            preview
                .clone()
                .cached(gpui::StyleRefinement::default().absolute().inset_0()),
        );
    }
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

/// The meta line ("2M views • 3 days ago"), or "Premieres in 3 hours" for an upcoming video.
/// Core words `published` the way YouTube does (§0b), transcript matches included.
fn meta_line(v: &Video) -> String {
    match &v.upcoming {
        Some(upcoming) => upcoming.clone(),
        None => v.meta_line(),
    }
}

fn verified(
    name: impl Into<SharedString>,
    size: f32,
    lh: f32,
    weight: gpui::FontWeight,
    color: gpui::Hsla,
    on: bool,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w(px(0.0))
        .child(line(name, size, lh, weight, color).min_w(px(0.0)))
        .when(on, |d| d.child(icon("check_circle", size, color)))
}

fn video_badges(v: &Video) -> Option<Div> {
    let mut tags = v.badges.iter().filter(|b| b.as_str() != "LIVE").peekable();
    tags.peek()?;
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(tags.take(5).map(|label| {
                txt(label.clone(), 11.0, 16.0, W500, text2())
                    .px(px(4.0))
                    .rounded(px(2.0))
                    .bg(fill())
            })),
    )
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
        .children(target(cx, Layer::Page, "more", "More actions"))
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
        probe::begin(cx, &[Layer::Page]);
        let geo = self.geo;
        let t = now();
        let dynamic = self
            .top()
            .map(|p| p.home_chips.as_slice())
            .unwrap_or_default();
        let labels: Vec<String> = if !self.top().is_some_and(Page::is_home) {
            Vec::new()
        } else if dynamic.is_empty() {
            TOPICS.iter().map(|t| t.title.to_owned()).collect()
        } else {
            dynamic.iter().map(|t| t.label.clone()).collect()
        };
        if self.chip_widths.len() != labels.len() {
            self.chip_widths = labels
                .iter()
                .map(|label| measure_width(window, label, 14.0, 500) + 24.0)
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
                let pre = if watch {
                    Vec::new()
                } else {
                    lay.visible(offset - vh, offset + 2.0 * vh)
                        .filter(|bl| bl.y + bl.h <= a || bl.y >= b)
                        .cloned()
                        .collect()
                };
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
            tab: false,
            request_images: true,
            preview: self
                .hover_video
                .as_ref()
                .filter(|(id, _)| *id == page.id)
                .map(|(_, index)| (*index, self.preview.clone())),
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
            ctx.tab = b.y + b.h > offset && b.y < offset + vh;
            // Related rows are cheap to build, but rapid Watch navigation rarely visits the
            // second viewport. Give visible posters and one or two rows of lead time priority.
            ctx.request_images =
                !watch || (b.y + b.h > offset - 144.0 && b.y < offset + vh + 144.0);
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
        let mut root = div()
            .id("content")
            .tab_group()
            .tab_index(4)
            .tab_stop(false)
            .relative()
            .size_full();
        if let Some(topic) = topic {
            root = root.child(self.chips_bar(topic, &labels, cw, geo.pad(), cx));
        }
        root.child(area).into_any_element()
    }

    fn chips_bar(
        &mut self,
        selected: usize,
        labels: &[String],
        cw: f32,
        pad: f32,
        cx: &mut Cx,
    ) -> AnyElement {
        let total: f32 = self.chip_widths.iter().sum::<f32>()
            + 12.0 * (self.chip_widths.len().saturating_sub(1)) as f32;
        let avail = cw - 2.0 * pad;
        self.chips_max = (total - avail).max(0.0);
        self.chips_x = self.chips_x.clamp(0.0, self.chips_max);
        let x = self.chips_x;
        let mut chip_x = pad - x;
        let mut row = div()
            .absolute()
            .top(px(12.0))
            .left(px(pad - x))
            .flex()
            .gap(px(12.0));
        for (i, label) in labels.iter().enumerate() {
            let on = i == selected;
            let width = self.chip_widths.get(i).copied().unwrap_or(0.0);
            let on_screen = chip_x + width > 0.0 && chip_x < cw;
            chip_x += width + 12.0;
            row = row.child(
                div()
                    .id(("chip", i))
                    .when(on_screen, |d| d.tab_index(0).focus_visible(focus_ring))
                    .children(target(cx, Layer::Page, "chip", label.as_str()))
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
                        label.clone(),
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
            retry_row("retry-more", "Couldn't load more", cx, NtApp::retry_more)
        }
        BlockKind::CommentsError => retry_row(
            "retry-comments",
            "Couldn't load comments",
            cx,
            NtApp::retry_comments,
        ),
        BlockKind::Empty(kind) => empty_state(kind, cx),
        BlockKind::Notice(i) => notice(ctx, *i, cx),
        BlockKind::SearchChips => search_chips(ctx, cx),
        BlockKind::SearchEntry(i) => search_entry(ctx, *i, b.w, b.h, cx),
        BlockKind::SectionLabel => div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(txt(
                "Matches in video transcripts · search.pronsh.dev",
                16.0,
                22.0,
                W500,
                text(),
            ))
            .into_any_element(),
        BlockKind::ContinueWatching => {
            let Some((v, at)) = &ctx.page.resume else {
                return div().into_any_element();
            };
            let art = ctx.image(Some(&v.thumbnail), 144.0, cx);
            let (video, start) = (v.clone(), *at);
            div()
                .id("continue-watching")
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .gap(px(16.0))
                .px(px(12.0))
                .rounded(px(12.0))
                .bg(fill())
                .hover(|s| s.bg(fill_hover()))
                .cursor_pointer()
                .when(ctx.tab, |d| d.tab_index(0).focus_visible(focus_ring))
                .children(target(cx, Layer::Page, "resume", v.title.as_str()))
                .child(picture(art, 144.0, 81.0, 8.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .child(txt(
                            format!("Continue watching · {}", nt_core::format::clock(start)),
                            12.0,
                            18.0,
                            W500,
                            text2(),
                        ))
                        .child(lines(v.title.clone(), 16.0, 22.0, W500, text(), 2)),
                )
                .on_click(cx.listener(move |a, _, window, cx| {
                    a.open_video_at(video.clone(), Some(start), window, cx)
                }))
                .into_any_element()
        }
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
        BlockKind::ChannelTabs => channel_tabs(ctx, cx),
        BlockKind::ChannelSort => channel_sort(ctx, cx),
        BlockKind::ChannelPlaylist(i) => {
            let Kind::Channel {
                info: Some(info), ..
            } = &ctx.page.kind
            else {
                return div().into_any_element();
            };
            let Some(playlist) = info.playlists.items.get(*i) else {
                return div().into_any_element();
            };
            playlist_result(ctx, playlist, *i, b.w, b.h, cx)
        }
        BlockKind::SubsStrip => subs_strip(ctx, b.w, cx),
        BlockKind::Player => div().into_any_element(),
        BlockKind::WatchMeta { title_h } => watch_meta(ctx, b.w, *title_h, cx),
        BlockKind::WatchDesc { lines, shown } => watch_desc(ctx, *lines, *shown, cx),
        BlockKind::Chapter(i) => chapter_row(ctx, *i, cx),
        BlockKind::RelatedChips => related_chips(ctx, cx),
        BlockKind::PlaylistHeader => ctx
            .page
            .watch()
            .and_then(|w| w.info.as_ref())
            .and_then(|i| i.playlist.as_ref())
            .map(|p| {
                line(p.title.clone(), 16.0, 22.0, W700, text())
                    .pt(px(8.0))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        BlockKind::PlaylistVideo(i) => watch_playlist_row(ctx, *i, cx),
        BlockKind::PlaylistMore => {
            let Some(w) = ctx.page.watch() else {
                return div().into_any_element();
            };
            let id = ctx.page.id;
            if w.playlist_loading {
                spinner_block(ctx, b)
            } else {
                let label = if w.playlist_error {
                    "Couldn't load more · Retry"
                } else {
                    "Show more videos"
                };
                pill("playlist-more", false)
                    .child(txt(label, 14.0, 20.0, W500, text()))
                    .children(target(cx, Layer::Page, "playlist_more", label))
                    .on_click(cx.listener(move |a, _, _, cx| a.more_watch_playlist(id, cx)))
                    .into_any_element()
            }
        }
        BlockKind::CommentsHeader => comments_header(ctx, cx),
        BlockKind::Comment { i, lines, shown } => {
            let Some(w) = ctx.page.watch() else {
                return div().into_any_element();
            };
            let Some(c) = w.comments.get(*i) else {
                return div().into_any_element();
            };
            comment(ctx, c, Some(*i), b.w, *lines, *shown, cx)
        }
        BlockKind::Reply {
            parent,
            index,
            lines,
        } => {
            let Some(c) = ctx
                .page
                .watch()
                .and_then(|w| w.replies.get(parent))
                .and_then(|r| r.items.get(*index))
            else {
                return div().into_any_element();
            };
            comment(ctx, c, None, b.w, *lines, *lines, cx)
        }
        BlockKind::ReplyTail(i) => {
            let Some(r) = ctx.page.watch().and_then(|w| w.replies.get(i)) else {
                return div().into_any_element();
            };
            if r.status == CommentsState::Loading {
                return spinner_block(ctx, b);
            }
            let i = *i;
            let label = if matches!(r.status, CommentsState::Failed(_)) {
                "Couldn't load replies · Retry"
            } else {
                "Show more replies"
            };
            pill(("replies-more", i), false)
                .children(target(cx, Layer::Page, "comment_replies", label))
                .child(txt(label, 14.0, 20.0, W500, link()))
                .on_click(cx.listener(move |a, _, _, cx| a.more_replies(i, cx)))
                .into_any_element()
        }
    }
}

fn retry_row(
    id: &'static str,
    label: &'static str,
    cx: &mut Cx,
    retry: fn(&mut NtApp, &mut Cx),
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .child(txt(label, 14.0, 20.0, W400, text2()))
        .child(
            pill(id, false)
                .children(target(cx, Layer::Page, "retry", "Retry"))
                .child(txt("Retry", 14.0, 20.0, W500, link()))
                .on_click(cx.listener(move |a, _, _, cx| retry(a, cx))),
        )
        .into_any_element()
}

fn watch_playlist_row(ctx: &mut Ctx, i: usize, cx: &mut Cx) -> AnyElement {
    let Some(panel) = ctx
        .page
        .watch()
        .and_then(|w| w.info.as_ref())
        .and_then(|info| info.playlist.as_ref())
    else {
        return div().into_any_element();
    };
    let Some(v) = panel.items.get(i) else {
        return div().into_any_element();
    };
    let on = panel.index == Some(i);
    let art = ctx.image(Some(&v.thumbnail), 112.0, cx);
    div()
        .id(("playlist-video", i))
        .size_full()
        .flex()
        .items_center()
        .gap(px(10.0))
        .p(px(4.0))
        .rounded(px(8.0))
        .when(on, |d| d.bg(fill()))
        .when(!on, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(fill()))
                .tab_index(0)
                .focus_visible(focus_ring)
                .children(target(cx, Layer::Page, "playlist_video", v.title.as_str()))
                .on_click(
                    cx.listener(move |a, _, window, cx| a.open_watch_playlist_row(i, window, cx)),
                )
        })
        .child(picture(art, 112.0, 63.0, 6.0))
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .child(lines(v.title.clone(), 14.0, 20.0, W500, text(), 2))
                .child(line(v.channel.clone(), 12.0, 18.0, W400, text2())),
        )
        .into_any_element()
}

fn related_chips(ctx: &Ctx, cx: &mut Cx) -> AnyElement {
    let Some(w) = ctx.page.watch() else {
        return div().into_any_element();
    };
    let Some(info) = &w.info else {
        return div().into_any_element();
    };
    div()
        .id("related-chips")
        .w_full()
        .h_full()
        .overflow_x_scroll()
        .flex()
        .items_center()
        .gap(px(8.0))
        .children(info.related_chips.iter().enumerate().map(|(i, chip)| {
            let on = w
                .related_selected
                .map_or(chip.selected, |active| active == i);
            pill(("related-chip", i), on)
                .children(target(cx, Layer::Page, "chip", chip.label.as_str()))
                .child(
                    txt(
                        chip.label.clone(),
                        14.0,
                        20.0,
                        W500,
                        if on { on_light() } else { text() },
                    )
                    .whitespace_nowrap(),
                )
                .on_click(cx.listener(move |a, _, _, cx| a.select_related(i, cx)))
        }))
        .into_any_element()
}

fn search_chips(ctx: &Ctx, cx: &mut Cx) -> AnyElement {
    let Kind::Search { view, .. } = &ctx.page.kind else {
        return div().into_any_element();
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .id("search-chips")
                .flex_1()
                .min_w(px(0.0))
                .overflow_x_scroll()
                .flex()
                .gap(px(8.0))
                .children(view.chips.iter().enumerate().map(|(i, option)| {
                    let filter = option.filter.clone();
                    pill(("search-chip", i), option.selected)
                        .child(
                            txt(
                                option.label.clone(),
                                14.0,
                                20.0,
                                W500,
                                if option.selected { on_light() } else { text() },
                            )
                            .whitespace_nowrap(),
                        )
                        .when_some(filter, |d, filter| {
                            d.children(target(cx, Layer::Page, "chip", option.label.as_str()))
                                .on_click(cx.listener(move |a, _, window, cx| {
                                    a.filter_search(filter.clone(), window, cx)
                                }))
                        })
                })),
        )
        .when(!view.filters.is_empty(), |d| {
            d.child(
                pill("search-filters", false)
                    .children(target(cx, Layer::Page, "filters", "Filters"))
                    .child(icon("tune", 20.0, text()))
                    .child(txt("Filters", 14.0, 20.0, W500, text()))
                    .on_click(cx.listener(|a, _, _, cx| a.set_overlay(Overlay::Filters, cx))),
            )
        })
        .into_any_element()
}

fn search_entry(ctx: &mut Ctx, i: usize, w: f32, h: f32, cx: &mut Cx) -> AnyElement {
    let Kind::Search { view, .. } = &ctx.page.kind else {
        return div().into_any_element();
    };
    let tw = list_thumb_w(w);
    match view.rows.get(i) {
        Some(SearchRow::Shelf(title)) => div()
            .size_full()
            .border_t_1()
            .border_color(divider())
            .flex()
            .items_center()
            .child(txt(title.clone(), 18.0, 26.0, W500, text()))
            .into_any_element(),
        Some(SearchRow::Playlist(p)) => playlist_result(ctx, p, i, w, h, cx),
        Some(SearchRow::Channel(c)) => {
            let art = ctx.image(c.channel.avatar.as_deref(), 112.0, cx);
            let id = c.channel.id.clone();
            let meta = [c.handle.as_deref(), c.channel.subscribers.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" • ");
            div()
                .id(("search-channel", i))
                .size_full()
                .flex()
                .items_center()
                .gap(px(16.0))
                .cursor_pointer()
                .when(ctx.tab, |d| d.tab_index(0).focus_visible(focus_ring))
                .children(target(cx, Layer::Page, "row", c.channel.name.as_str()))
                .on_click(
                    cx.listener(move |a, _, window, cx| a.open_channel(id.clone(), window, cx)),
                )
                .child(
                    div()
                        .w(px(tw))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(avatar(art, 112.0)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(verified(
                            c.channel.name.clone(),
                            18.0,
                            26.0,
                            W400,
                            text(),
                            c.verified,
                        ))
                        .child(line(meta, 12.0, 18.0, W400, text2())),
                )
                .into_any_element()
        }
        _ => div().into_any_element(),
    }
}

fn playlist_result(
    ctx: &mut Ctx,
    playlist: &PlaylistRef,
    i: usize,
    w: f32,
    h: f32,
    cx: &mut Cx,
) -> AnyElement {
    let tw = list_thumb_w(w);
    let art = ctx.image(playlist.thumbnail.as_deref(), tw, cx);
    let id = playlist.id.clone();
    div()
        .id(("search-playlist", i))
        .size_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .tab_index(0)
        .focus_visible(focus_ring)
        .children(target(cx, Layer::Page, "playlist", playlist.title.as_str()))
        .on_click(cx.listener(move |a, _, _, cx| a.open_playlist(&id, cx)))
        .gap(px(16.0))
        .child(
            div()
                .w(px(tw))
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(12.0))
                        .right(px(12.0))
                        .top(px(-5.0))
                        .h(px(12.0))
                        .rounded(px(8.0))
                        .bg(fill()),
                )
                .child(picture(art, tw, h.min(tw * 9.0 / 16.0), 12.0))
                .when_some(playlist.video_count.clone(), |d, count| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(8.0))
                            .bottom(px(8.0))
                            .bg(badge_bg())
                            .rounded(px(4.0))
                            .px(px(6.0))
                            .child(txt(count, 12.0, 18.0, W500, text())),
                    )
                }),
        )
        .child(lines(playlist.title.clone(), 18.0, 26.0, W500, text(), 2))
        .into_any_element()
}

fn channel_tabs(ctx: &Ctx, cx: &mut Cx) -> AnyElement {
    let Kind::Channel {
        info: Some(info),
        tab,
        ..
    } = &ctx.page.kind
    else {
        return div().into_any_element();
    };
    let fallback = [ChannelTab::Videos];
    let tabs = if info.tabs.is_empty() {
        &fallback[..]
    } else {
        &info.tabs
    };
    div()
        .size_full()
        .border_b_1()
        .border_color(divider())
        .flex()
        .items_end()
        .children(tabs.iter().copied().enumerate().map(|(i, choice)| {
            let label = match choice {
                ChannelTab::Home => "Home",
                ChannelTab::Videos => "Videos",
                ChannelTab::Live => "Live",
                ChannelTab::Playlists => "Playlists",
            };
            div()
                .id(("channel-tab", i))
                .h(px(48.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .tab_index(0)
                .focus_visible(focus_ring)
                .when(choice == *tab, |d| d.border_b_2().border_color(text()))
                .children(target(cx, Layer::Page, "tab", label))
                .child(txt(
                    label,
                    16.0,
                    22.0,
                    W500,
                    if choice == *tab { text() } else { text2() },
                ))
                .on_click(cx.listener(move |a, _, _, cx| a.channel_tab(choice, cx)))
        }))
        .into_any_element()
}

fn channel_sort(ctx: &Ctx, cx: &mut Cx) -> AnyElement {
    let Kind::Channel {
        info: Some(info),
        sort,
        ..
    } = &ctx.page.kind
    else {
        return div().into_any_element();
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .gap(px(8.0))
        .children(info.video_sort.iter().enumerate().map(|(i, chip)| {
            let on = sort.map_or(chip.selected, |s| s == i);
            pill(("channel-sort", i), on)
                .children(target(cx, Layer::Page, "chip", chip.label.as_str()))
                .child(txt(
                    chip.label.clone(),
                    14.0,
                    20.0,
                    W500,
                    if on { on_light() } else { text() },
                ))
                .on_click(cx.listener(move |a, _, _, cx| a.channel_sort(i, cx)))
        }))
        .into_any_element()
}

fn chapter_row(ctx: &mut Ctx, i: usize, cx: &mut Cx) -> AnyElement {
    let Some(chapter) = ctx
        .page
        .watch()
        .and_then(|w| w.info.as_ref())
        .and_then(|info| info.chapters.get(i))
    else {
        return div().into_any_element();
    };
    let start = chapter.start;
    let art = ctx.image(chapter.thumbnail.as_deref(), 84.0, cx);
    div()
        .id(("chapter", i))
        .h_full()
        .w_full()
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .rounded(px(8.0))
        .bg(fill())
        .hover(|s| s.bg(fill_hover()))
        .cursor_pointer()
        .tab_index(0)
        .focus_visible(focus_ring)
        .children(target(cx, Layer::Page, "chapter", chapter.title.as_str()))
        .child(picture(art, 84.0, 48.0, 6.0))
        .child(txt(nt_core::format::clock(start), 14.0, 20.0, W500, link()))
        .child(line(chapter.title.clone(), 14.0, 20.0, W500, text()))
        .on_click(cx.listener(move |a, _, _, cx| a.player.update(cx, |p, cx| p.seek_to(start, cx))))
        .into_any_element()
}

fn comments_header(ctx: &Ctx, cx: &mut Cx) -> AnyElement {
    let Some(w) = ctx.page.watch() else {
        return div().into_any_element();
    };
    let title = w
        .comments_total
        .as_ref()
        .map_or_else(|| "Comments".to_string(), |n| format!("{n} Comments"));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.0))
                .child(txt(title, 20.0, 28.0, W700, text()))
                .when(!w.comments_sort.is_empty(), |d| {
                    d.child(
                        pill("comments-sort", false)
                            .children(target(cx, Layer::Page, "sort", "Sort by"))
                            .child(icon("tune", 20.0, text()))
                            .child(txt("Sort by", 14.0, 20.0, W500, text()))
                            .on_click(cx.listener(|a, _, _, cx| a.toggle_comment_sort(cx))),
                    )
                }),
        )
        .when(w.sort_open, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .mt(px(8.0))
                    .children(w.comments_sort.iter().enumerate().map(|(i, sort)| {
                        pill(("comment-sort", i), sort.selected)
                            .children(target(cx, Layer::Page, "menu_item", sort.label.as_str()))
                            .child(txt(
                                sort.label.clone(),
                                14.0,
                                20.0,
                                W500,
                                if sort.selected { on_light() } else { text() },
                            ))
                            .on_click(cx.listener(move |a, _, _, cx| a.sort_comments(i, cx)))
                    })),
            )
        })
        .into_any_element()
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

/// Highlights must be sorted and must not overlap.
fn sorted_disjoint(
    mut v: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    v.sort_by_key(|(r, _)| r.start);
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for (r, st) in v {
        match out.last_mut() {
            Some((last, _)) if r.start < last.end => last.end = last.end.max(r.end),
            _ => out.push((r, st)),
        }
    }
    out
}

/// A search link inside a notice ("Did you mean", "Search instead for").
fn query_link(id: (&'static str, usize), q: String, size: f32, cx: &mut Cx) -> impl IntoElement {
    let label = q.clone();
    div()
        .id(id)
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| this.submit_search(q.clone(), window, cx)))
        .child(
            txt(label, size, size + 6.0, W500, link())
                .italic()
                .hover(|s| s.underline()),
        )
}

fn notice(ctx: &mut Ctx, i: usize, cx: &mut Cx) -> AnyElement {
    let Kind::Search { view, .. } = &ctx.page.kind else {
        return div().into_any_element();
    };
    let Some(n) = view.notices.get(i).cloned() else {
        return div().into_any_element();
    };
    match n {
        nt_core::SearchNotice::AgeGated { title, body } => div()
            .size_full()
            .p(px(16.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(divider())
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(txt(title, 20.0, 28.0, W700, text()))
            .child(txt(body, 14.0, 20.0, W400, text2()))
            .into_any_element(),
        nt_core::SearchNotice::Corrected { showing, original } => div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(txt("Showing results for", 16.0, 22.0, W400, text()))
                    .child(query_link(("notice-show", i), showing, 16.0, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(txt("Search instead for", 14.0, 20.0, W400, text2()))
                    .child(query_link(("notice-orig", i), original, 14.0, cx)),
            )
            .into_any_element(),
        nt_core::SearchNotice::DidYouMean { suggestion } => div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(txt("Did you mean:", 16.0, 22.0, W400, text()))
            .child(query_link(("notice-dym", i), suggestion, 16.0, cx))
            .into_any_element(),
        nt_core::SearchNotice::NoResults => div().into_any_element(),
    }
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
    let pic = thumb(ctx, v, (w, th), 12.0, index, cx);
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
        .child(verified(
            v.channel.clone(),
            14.0,
            20.0,
            W400,
            text2(),
            v.channel_verified,
        ));
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
    // Hover: a #272727 plate bleeding 8 px around the card, radius 12 (YouTube's 2024 grid).
    let plate = div()
        .absolute()
        .top(px(-8.0))
        .left(px(-8.0))
        .right(px(-8.0))
        .bottom(px(-8.0))
        .rounded(px(12.0))
        .group_hover("card", |s| s.bg(c(0x272727)));
    let hover_id = v.id.clone();
    div()
        .id(("card", index))
        .on_hover(cx.listener(move |a, active: &bool, _, cx| {
            a.hover_card((page_id, index), &hover_id, w, *active, cx);
        }))
        .when(ctx.tab, |d| d.tab_index(0).focus_visible(focus_ring))
        .children(target(cx, Layer::Page, "card", v.title.as_str()))
        .group("card")
        .relative()
        .child(plate)
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
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .min_w(px(0.0))
                                .child(line(meta_line(v), 14.0, 20.0, W400, text2()).min_w(px(0.0)))
                                .children(video_badges(v)),
                        )
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
        Kind::Search { query, .. } => Some(query.as_str()),
        _ => None,
    };
    let Some(v) = ctx.page.feed.items.get(index) else {
        return div().into_any_element();
    };
    let tw = list_thumb_w(w);
    let th = (tw * 9.0 / 16.0).round();
    let pic = thumb(ctx, v, (tw, th), 12.0, index, cx);
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
        .child(verified(
            v.channel.clone(),
            12.0,
            18.0,
            W400,
            text2(),
            v.channel_verified,
        ));
    let moment = v.moment.as_ref().map(|m| {
        let label = if m.label.is_empty() {
            nt_core::format::clock(m.start_secs)
        } else {
            m.label.clone()
        };
        let chip = div()
            .h(px(24.0))
            .px(px(8.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .rounded(px(4.0))
            .bg(fill())
            .child(icon("play_arrow", 16.0, text()))
            .child(txt(label, 12.0, 18.0, W500, text()));
        let highlights: Vec<(Range<usize>, HighlightStyle)> = m
            .highlights
            .iter()
            .filter(|r| {
                r.start < r.end
                    && m.snippet.is_char_boundary(r.start)
                    && m.snippet.is_char_boundary(r.end.min(m.snippet.len()))
                    && r.end <= m.snippet.len()
            })
            .map(|r| {
                (
                    r.clone(),
                    HighlightStyle {
                        font_weight: Some(W700),
                        color: Some(text()),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let snippet = StyledText::new(SharedString::from(m.snippet.clone()))
            .with_highlights(sorted_disjoint(highlights));
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().flex().child(chip))
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(text2())
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(snippet),
            )
    });
    let snippet = v.snippet.as_ref().filter(|_| v.moment.is_none()).map(|s| {
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
    let hover_id = v.id.clone();
    div()
        .id(("row", index))
        .on_hover(cx.listener(move |a, active: &bool, _, cx| {
            a.hover_card((page_id, index), &hover_id, tw, *active, cx);
        }))
        .when(ctx.tab, |d| d.tab_index(0).focus_visible(focus_ring))
        .children(target(cx, Layer::Page, "row", v.title.as_str()))
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
                .child(line(meta_line(v), 12.0, 18.0, W400, text2()))
                .child(ch)
                .children(snippet)
                .children(video_badges(v))
                .children(moment)
                .child(more_button(("row-more", index), "row", page_id, index, cx)),
        )
        .into_any_element()
}

fn compact_row(ctx: &mut Ctx, index: usize, cx: &mut Cx) -> AnyElement {
    let page_id = ctx.page.id;
    let Some(v) = ctx.page.feed.items.get(index) else {
        return div().into_any_element();
    };
    let pic = thumb(ctx, v, (COMPACT_THUMB_W, COMPACT_THUMB_H), 8.0, index, cx);
    div()
        .id(("rel", index))
        .when(ctx.tab, |d| d.tab_index(0).focus_visible(focus_ring))
        .children(target(cx, Layer::Page, "related", v.title.as_str()))
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
                .child(verified(
                    v.channel.clone(),
                    12.0,
                    18.0,
                    W400,
                    text2(),
                    v.channel_verified,
                ))
                .child(line(meta_line(v), 12.0, 18.0, W400, text2()))
                .child(more_button(("rel-more", index), "rel", page_id, index, cx)),
        )
        .into_any_element()
}

fn subscribe_button(subscribed: bool, cx: &mut Cx) -> impl IntoElement {
    let b = pill("subscribe", !subscribed)
        .children(target(
            cx,
            Layer::Page,
            "subscribe",
            if subscribed {
                "Subscribed"
            } else {
                "Subscribe"
            },
        ))
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
    let desc_el = (!desc.is_empty() || info.about.is_some() || !info.links.is_empty()).then(|| {
        div()
            .id("channel-desc")
            .w(px(tw))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .tab_index(0)
            .focus_visible(focus_ring)
            .children(target(cx, Layer::Page, "about", "Channel info"))
            .on_click(cx.listener(|a, _, _, cx| a.open_channel_about(cx)))
            .child(
                line(desc, 14.0, 20.0, W400, text2())
                    .flex_shrink(1.0)
                    .min_w(px(0.0)),
            )
            .child(
                txt("...more", 14.0, 20.0, W500, text())
                    .flex_none()
                    .whitespace_nowrap(),
            )
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
                .child(verified(
                    info.name.clone(),
                    if small { 24.0 } else { 36.0 },
                    if small { 32.0 } else { 44.0 },
                    W700,
                    text(),
                    info.verified,
                ))
                .child(line(meta.join(" • "), 14.0, 20.0, W400, text2()).mt(px(8.0)))
                .children(desc_el.map(|d| d.mt(px(8.0))))
                .when_some(info.links_label.clone(), |d, label| {
                    d.child(
                        div()
                            .id("channel-links")
                            .cursor_pointer()
                            .tab_index(0)
                            .focus_visible(focus_ring)
                            .children(target(cx, Layer::Page, "about", label.as_str()))
                            .on_click(cx.listener(|a, _, _, cx| a.open_channel_about(cx)))
                            .child(line(label, 14.0, 20.0, W500, link())),
                    )
                })
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
                .child(verified(
                    name,
                    16.0,
                    22.0,
                    W500,
                    text(),
                    info.channel_verified,
                ))
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
        .children(target(cx, Layer::Page, "share", "Share"))
        .on_click(cx.listener(move |this, _, _, cx| this.copy_link(&vid, cx)))
        .child(icon("share", 24.0, text()))
        .when(!compact, |d| {
            d.child(txt("Share", 14.0, 20.0, W500, text()))
        });
    let later = pill("watch-later", false)
        .children(target(cx, Layer::Page, "watch_later", "Watch later"))
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
                    this.player.update(cx, |p, cx| p.seek_to(at, cx));
                }
            }
        });
    });
    let has_chapters = !info.chapters.is_empty();
    let more = (!expanded && (lines_ > DESC_CLAMP || has_chapters)).then(|| {
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
        .when(expanded && (lines_ > DESC_CLAMP || has_chapters), |d| {
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
    cm: &Comment,
    index: Option<usize>,
    w: f32,
    lines: usize,
    shown: usize,
    cx: &mut Cx,
) -> AnyElement {
    let av = ctx.image(cm.author_avatar.as_deref(), 40.0, cx);
    let creator = ctx
        .page
        .watch()
        .and_then(|w| w.info.as_ref())
        .map(|i| &i.channel);
    let creator_avatar = if cm.hearted {
        ctx.image(creator.and_then(|c| c.avatar.as_deref()), 20.0, cx)
    } else {
        None
    };
    let lines_ = lines;
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
        .child(icon("thumb_down", 20.0, text()).ml(px(12.0)))
        .when(cm.hearted, |d| {
            d.child(
                div()
                    .relative()
                    .ml(px(12.0))
                    .child(avatar(creator_avatar, 20.0))
                    .child(
                        div()
                            .absolute()
                            .right(px(-3.0))
                            .bottom(px(-3.0))
                            .child(icon("favorite", 12.0, red())),
                    ),
            )
        });
    let expanded_replies = index
        .and_then(|i| ctx.page.watch().and_then(|w| w.replies.get(&i)))
        .is_some_and(|r| r.expanded);
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
                .when(index.is_some() && cm.pinned, |d| {
                    d.child(
                        txt(
                            format!(
                                "Pinned by {}",
                                creator.map_or("the creator", |c| c.name.as_str())
                            ),
                            12.0,
                            18.0,
                            W500,
                            text2(),
                        )
                        .h(px(22.0)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(4.0))
                        .child(verified(
                            cm.author.clone(),
                            13.0,
                            18.0,
                            W500,
                            text(),
                            cm.author_verified,
                        ))
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
                .when_some(index.filter(|_| clamped), |d, i| {
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
                .child(actions)
                .when_some(index.filter(|_| cm.replies.is_some()), |d, i| {
                    let label = match cm.reply_count.as_deref() {
                        Some("1") => "1 reply".to_string(),
                        Some(n) => format!("{n} replies"),
                        None => "Replies".to_string(),
                    };
                    d.child(
                        div()
                            .id(("replies", i))
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .cursor_pointer()
                            .tab_index(0)
                            .focus_visible(focus_ring)
                            .children(target(cx, Layer::Page, "comment_replies", label.as_str()))
                            .child(icon(
                                if expanded_replies {
                                    "expand_less"
                                } else {
                                    "expand_more"
                                },
                                20.0,
                                link(),
                            ))
                            .child(txt(label, 14.0, 20.0, W500, link()))
                            .on_click(cx.listener(move |a, _, _, cx| a.toggle_replies(i, cx))),
                    )
                }),
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
            crate::player::trace("card-click", &v.id);
            // A transcript match plays from its moment.
            let at = v.moment.as_ref().map(|m| m.start_secs);
            self.open_video_at(v, at, window, cx);
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
