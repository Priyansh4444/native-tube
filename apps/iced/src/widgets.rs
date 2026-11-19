//! Small custom widgets iced doesn't ship:
//! - `Clamp`: rich text clamped to N lines with an ellipsis or an inline "…more", clickable
//!   link spans, and a process-wide cache of where to cut (iced has no line clamp);
//! - `Thumb`: a thumbnail with its duration/LIVE badge, resume bar and press darkening;
//! - `Spinner`, `SeekBar` (the player scrubber), `SearchBox` (search-field focus and keys);
//! - `AutoHide` (player controls and cursor that hide after 3 s), `Bezel` (the centre
//!   play/pause flash), `ChipBar` (the sticky, horizontally scrollable topic chips).

use crate::icons::Icon;
use crate::theme as t;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self as atext, Paragraph as _, Span};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse};
use iced::alignment;
use iced::keyboard::{self, key::Named};
use iced::widget::image::Handle;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{
    Border, Color, Element, Event, Font, Length, Pixels, Point, Radians, Rectangle, Renderer, Size,
    Theme, Vector, window,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

type Paragraph = <Renderer as atext::Renderer>::Paragraph;

fn text_spec(content: &str, width: f32, style: t::Style) -> atext::Text<&str> {
    atext::Text {
        content,
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(style.size),
        line_height: LineHeight::Absolute(Pixels(style.line)),
        font: style.font,
        align_x: atext::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::WordOrGlyph,
    }
}

fn draw_icon(renderer: &mut Renderer, icon: Icon, bounds: Rectangle, color: Color) {
    use iced::advanced::svg::Renderer as _;
    renderer.draw_svg(
        iced::advanced::svg::Svg {
            handle: icon.handle(),
            color: Some(color),
            rotation: Radians(0.0),
            opacity: color.a,
        },
        bounds,
        bounds,
    );
}

fn fill(renderer: &mut Renderer, bounds: Rectangle, radius: f32, color: Color) {
    use iced::advanced::Renderer as _;
    renderer.fill_quad(
        Quad {
            bounds,
            border: Border {
                radius: radius.into(),
                ..Border::default()
            },
            ..Quad::default()
        },
        color,
    );
}

// ---------------------------------------------------------------------------------------------
// Clamped rich text

/// One run of a `Clamp`: its text, weight, colour, and an optional link id.
#[derive(Debug, Clone)]
pub struct Seg<'a> {
    pub text: Cow<'a, str>,
    pub font: Option<Font>,
    pub color: Option<Color>,
    pub link: Option<usize>,
}

pub fn seg<'a>(text: impl Into<Cow<'a, str>>) -> Seg<'a> {
    Seg {
        text: text.into(),
        font: None,
        color: None,
        link: None,
    }
}

impl<'a> Seg<'a> {
    pub fn font(mut self, f: Font) -> Self {
        self.font = Some(f);
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn link(mut self, id: usize, color: Color) -> Self {
        self.link = Some(id);
        self.color = Some(color);
        self
    }
}

/// Text that wraps to at most `lines` lines (0 = no limit). When it is cut, `suffix` is
/// appended ("…" by default, or "…more"). Link segments publish `on_link(id)`; a click
/// elsewhere publishes `on_press`.
pub struct Clamp<'a, M> {
    segs: Vec<Seg<'a>>,
    style: t::Style,
    lines: usize,
    color: Option<Color>,
    suffix: Option<(&'static str, Font, Color)>,
    on_link: Option<Box<dyn Fn(usize) -> M + 'a>>,
    on_press: Option<M>,
}

pub fn clamp<'a, M>(
    content: impl Into<Cow<'a, str>>,
    style: t::Style,
    lines: usize,
) -> Clamp<'a, M> {
    rich(vec![seg(content)], style, lines)
}

pub fn rich<'a, M>(segs: Vec<Seg<'a>>, style: t::Style, lines: usize) -> Clamp<'a, M> {
    Clamp {
        segs,
        style,
        lines,
        color: None,
        suffix: None,
        on_link: None,
        on_press: None,
    }
}

impl<'a, M> Clamp<'a, M> {
    /// Without a colour the text takes the enclosing style's (so links can change on hover).
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// What to append when the text is cut (default "…").
    pub fn suffix(mut self, text: &'static str, font: Font, color: Color) -> Self {
        self.suffix = Some((text, font, color));
        self
    }
    pub fn on_link(mut self, f: impl Fn(usize) -> M + 'a) -> Self {
        self.on_link = Some(Box::new(f));
        self
    }
    pub fn on_press(mut self, m: M) -> Self {
        self.on_press = Some(m);
        self
    }

    fn key(&self, width: f32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &self.segs {
            s.text.hash(&mut h);
            s.font.map(|f| f.weight as u8).hash(&mut h);
        }
        self.lines.hash(&mut h);
        self.style.size.to_bits().hash(&mut h);
        width.to_bits().hash(&mut h);
        self.suffix.map(|s| s.0).hash(&mut h);
        h.finish()
    }
}

struct ClampState {
    key: u64,
    para: Option<Paragraph>,
    /// Link id of each span (None for plain spans).
    links: Vec<Option<usize>>,
    truncated: bool,
    hovered_link: bool,
}

thread_local! {
    /// (text, width, lines, style) -> byte length kept. Shaping is the expensive part of a
    /// clamp; rows that come back into the band reuse the cut instead of searching again.
    static CUTS: RefCell<HashMap<u64, Option<usize>>> = RefCell::new(HashMap::new());
}
const CUTS_CAP: usize = 8192;

/// Build the spans for the first `cut` bytes of the concatenated text, plus the suffix.
fn build_spans<'s>(
    segs: &'s [Seg<'_>],
    cut: Option<usize>,
    suffix: (&'static str, Font, Color),
    base: Font,
) -> (Vec<Span<'s, (), Font>>, Vec<Option<usize>>) {
    let mut spans = Vec::with_capacity(segs.len() + 1);
    let mut links = Vec::with_capacity(segs.len() + 1);
    let mut left = cut.unwrap_or(usize::MAX);
    for s in segs {
        if left == 0 {
            break;
        }
        let text: &str = &s.text;
        let take = if text.len() <= left {
            text
        } else {
            let mut i = left;
            while i > 0 && !text.is_char_boundary(i) {
                i -= 1;
            }
            text.get(..i).unwrap_or("")
        };
        left = left.saturating_sub(text.len());
        let take = if cut.is_some() && left == 0 {
            take.trim_end()
        } else {
            take
        };
        let mut span = Span::new(take).font(s.font.unwrap_or(base));
        if let Some(c) = s.color {
            span = span.color(c);
        }
        spans.push(span);
        links.push(s.link);
    }
    if cut.is_some() {
        spans.push(Span::new(suffix.0).font(suffix.1).color(suffix.2));
        links.push(None);
    }
    (spans, links)
}

fn shape(spans: &[Span<'_, (), Font>], width: f32, style: t::Style) -> Paragraph {
    Paragraph::with_spans(atext::Text {
        content: spans,
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(style.size),
        line_height: LineHeight::Absolute(Pixels(style.line)),
        font: style.font,
        align_x: atext::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::WordOrGlyph,
    })
}

impl<'a, M: Clone> Clamp<'a, M> {
    fn fit(&self, width: f32) -> (Paragraph, Vec<Option<usize>>, bool) {
        let suffix = self
            .suffix
            .unwrap_or(("…", self.style.font, self.color.unwrap_or(t::TEXT)));
        let max_h = self.style.line * self.lines as f32 + 0.5;
        let key = self.key(width);
        let total: usize = self.segs.iter().map(|s| s.text.len()).sum();
        let cached = CUTS.with(|c| c.borrow().get(&key).copied());
        let cut = match cached {
            Some(cut) => cut,
            None => {
                let (spans, _) = build_spans(&self.segs, None, suffix, self.style.font);
                let full = shape(&spans, width, self.style);
                let cut = if self.lines == 0 || full.min_bounds().height <= max_h || total == 0 {
                    None
                } else {
                    // Longest prefix that fits with the suffix. Nothing past ~5 chars per em
                    // per line can fit, which keeps the search short.
                    let bound =
                        ((self.lines as f32 * width / (self.style.size * 0.2)) as usize).min(total);
                    let (mut lo, mut hi) = (0usize, bound);
                    while lo < hi {
                        let mid = (lo + hi).div_ceil(2);
                        let (spans, _) =
                            build_spans(&self.segs, Some(mid), suffix, self.style.font);
                        if shape(&spans, width, self.style).min_bounds().height <= max_h {
                            lo = mid;
                        } else {
                            hi = mid - 1;
                        }
                    }
                    Some(lo)
                };
                CUTS.with(|c| {
                    let mut c = c.borrow_mut();
                    if c.len() >= CUTS_CAP {
                        c.clear();
                    }
                    c.insert(key, cut);
                });
                cut
            }
        };
        let (spans, links) = build_spans(&self.segs, cut, suffix, self.style.font);
        (shape(&spans, width, self.style), links, cut.is_some())
    }
}

impl<'a, M: Clone + 'a> Widget<M, Theme, Renderer> for Clamp<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ClampState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ClampState {
            key: 0,
            para: None,
            links: Vec::new(),
            truncated: false,
            hovered_link: false,
        })
    }
    fn layout(&mut self, tree: &mut Tree, _r: &Renderer, limits: &layout::Limits) -> layout::Node {
        let width = limits.max().width.max(1.0);
        let key = self.key(width);
        let st = tree.state.downcast_mut::<ClampState>();
        if st.para.is_none() || st.key != key {
            let (para, links, truncated) = self.fit(width);
            st.para = Some(para);
            st.links = links;
            st.truncated = truncated;
            st.key = key;
        }
        let h = st
            .para
            .as_ref()
            .map(|p| p.min_bounds().height)
            .unwrap_or(0.0);
        layout::Node::new(Size::new(width, h))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        if self.on_link.is_none() && self.on_press.is_none() {
            return;
        }
        let st = tree.state.downcast_mut::<ClampState>();
        let b = layout.bounds();
        let link_at = |st: &ClampState, p: Point| {
            st.para
                .as_ref()
                .and_then(|para| para.hit_span(Point::new(p.x - b.x, p.y - b.y)))
                .and_then(|i| st.links.get(i).copied().flatten())
        };
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let hovered = cursor
                    .position_over(b)
                    .and_then(|p| link_at(st, p))
                    .is_some();
                if hovered != st.hovered_link {
                    st.hovered_link = hovered;
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(b) {
                    if let (Some(id), Some(f)) = (link_at(st, p), &self.on_link) {
                        shell.publish(f(id));
                        shell.capture_event();
                    } else if let Some(m) = &self.on_press {
                        shell.publish(m.clone());
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let st = tree.state.downcast_ref::<ClampState>();
        if cursor.is_over(layout.bounds()) && (st.hovered_link || self.on_press.is_some()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::text::Renderer as _;
        let st = tree.state.downcast_ref::<ClampState>();
        if let Some(p) = &st.para {
            renderer.fill_paragraph(
                p,
                layout.bounds().position(),
                self.color.unwrap_or(style.text_color),
                *viewport,
            );
        }
    }
}

impl<'a, M: Clone + 'a> From<Clamp<'a, M>> for Element<'a, M> {
    fn from(c: Clamp<'a, M>) -> Self {
        Element::new(c)
    }
}

/// Split `text` into segments with every case-insensitive occurrence of a query term set in
/// `hi_font`/`hi_color` (search snippets).
pub fn highlight<'a>(
    text: &'a str,
    terms: &[String],
    hi_font: Font,
    hi_color: Color,
) -> Vec<Seg<'a>> {
    let lower = text.to_lowercase();
    if lower.len() != text.len() || terms.is_empty() {
        return vec![seg(text)];
    }
    let mut marks = vec![false; text.len()];
    for term in terms.iter().filter(|t| t.len() >= 2) {
        let mut from = 0;
        while let Some(i) = lower.get(from..).and_then(|s| s.find(term.as_str())) {
            let start = from + i;
            let end = start + term.len();
            for m in marks.iter_mut().take(end).skip(start) {
                *m = true;
            }
            from = end;
        }
    }
    let mut segs = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let on = marks.get(start).copied().unwrap_or(false);
        let mut end = start + 1;
        while end < text.len()
            && (marks.get(end).copied().unwrap_or(false) == on || !text.is_char_boundary(end))
        {
            end += 1;
        }
        let piece = text.get(start..end).unwrap_or("");
        segs.push(if on {
            seg(piece).font(hi_font).color(hi_color)
        } else {
            seg(piece)
        });
        start = end;
    }
    segs
}

// ---------------------------------------------------------------------------------------------
// Thumbnail

/// A fixed-size image with a fill placeholder, rounded corners, an optional badge, a resume
/// bar and a 10% darkening while pressed. Badge, bar and darkening are drawn as stretched 1x1
/// images so they land above the thumbnail within the same layer (iced draws a layer's quads
/// before its images and its text after them).
pub struct Thumb {
    handle: Option<Handle>,
    width: f32,
    height: f32,
    radius: f32,
    badge: Option<(String, bool)>,
    progress: Option<f32>,
    cover: bool,
}

pub fn thumb(handle: Option<Handle>, width: f32, height: f32, radius: f32) -> Thumb {
    Thumb {
        handle,
        width: width.max(1.0),
        height: height.max(1.0),
        radius,
        badge: None,
        progress: None,
        cover: false,
    }
}

impl Thumb {
    /// `live` paints the badge red.
    pub fn badge(mut self, text: Option<String>, live: bool) -> Self {
        self.badge = text.map(|t| (t, live));
        self
    }
    pub fn progress(mut self, p: Option<f32>) -> Self {
        self.progress = p.filter(|p| p.is_finite() && *p > 0.0);
        self
    }
    /// Crop to fill instead of stretching.
    pub fn cover(mut self) -> Self {
        self.cover = true;
        self
    }
}

struct ThumbState {
    badge: Option<(String, Paragraph)>,
    pressed: bool,
}

impl<M> Widget<M, Theme, Renderer> for Thumb {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width), Length::Fixed(self.height))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ThumbState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ThumbState {
            badge: None,
            pressed: false,
        })
    }
    fn layout(&mut self, tree: &mut Tree, _r: &Renderer, limits: &layout::Limits) -> layout::Node {
        let st = tree.state.downcast_mut::<ThumbState>();
        match &self.badge {
            Some((text, _)) => {
                if st.badge.as_ref().is_none_or(|(s, _)| s != text) {
                    let mut spec = text_spec(text, f32::INFINITY, t::META_S_M);
                    spec.wrapping = Wrapping::None;
                    st.badge = Some((text.clone(), Paragraph::with_text(spec)));
                }
            }
            None => st.badge = None,
        }
        layout::Node::new(limits.resolve(
            self.width,
            self.height,
            Size::new(self.width, self.height),
        ))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<ThumbState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.is_over(layout.bounds()) =>
            {
                st.pressed = true;
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if st.pressed => {
                st.pressed = false;
                shell.request_redraw();
            }
            _ => {}
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::image::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let b = layout.bounds();
        let st = tree.state.downcast_ref::<ThumbState>();
        match &self.handle {
            Some(h) => {
                let mut draw_bounds = b;
                if self.cover
                    && let Some(sz) = renderer.measure_image(h)
                    && sz.width > 0
                    && sz.height > 0
                {
                    let s = (b.width / sz.width as f32).max(b.height / sz.height as f32);
                    let (w, hh) = (sz.width as f32 * s, sz.height as f32 * s);
                    draw_bounds = Rectangle {
                        x: b.x + (b.width - w) / 2.0,
                        y: b.y + (b.height - hh) / 2.0,
                        width: w,
                        height: hh,
                    };
                }
                let mut img = iced::advanced::image::Image::new(h.clone());
                img.border_radius = self.radius.into();
                renderer.draw_image(img, draw_bounds, b);
            }
            None => fill(renderer, b, self.radius, t::FILL_SOLID),
        }
        let image = |renderer: &mut Renderer, h: Handle, r: Rectangle, radius: f32| {
            let mut img = iced::advanced::image::Image::new(h);
            img.border_radius = radius.into();
            renderer.draw_image(img, r, r);
        };
        if st.pressed {
            image(renderer, t::press_pixel(), b, self.radius);
        }
        if let Some(p) = self.progress {
            let bar = Rectangle {
                y: b.y + b.height - 4.0,
                height: 4.0,
                ..b
            };
            let bottom = iced::border::Radius {
                top_left: 0.0,
                top_right: 0.0,
                bottom_left: self.radius,
                bottom_right: self.radius,
            };
            let mut track = iced::advanced::image::Image::new(t::track_pixel());
            track.border_radius = bottom;
            renderer.draw_image(track, bar, bar);
            let played = Rectangle {
                width: bar.width * p.clamp(0.0, 1.0),
                ..bar
            };
            let mut red = iced::advanced::image::Image::new(t::red_pixel());
            red.border_radius = iced::border::Radius {
                bottom_left: self.radius,
                ..iced::border::Radius::default()
            };
            renderer.draw_image(red, played, bar);
        }
        if let (Some((_, live)), Some((_, para))) = (&self.badge, &st.badge) {
            let sz = para.min_bounds();
            let (pw, ph) = (4.0, 1.0);
            let lift = if self.progress.is_some() { 4.0 } else { 0.0 };
            let r = Rectangle {
                x: b.x + b.width - 8.0 - sz.width - 2.0 * pw,
                y: b.y + b.height - 8.0 - lift - sz.height - 2.0 * ph,
                width: sz.width + 2.0 * pw,
                height: sz.height + 2.0 * ph,
            };
            let px = if *live {
                t::red_pixel()
            } else {
                t::badge_pixel()
            };
            image(renderer, px, r, 4.0);
            renderer.fill_paragraph(para, Point::new(r.x + pw, r.y + ph), t::TEXT, *viewport);
        }
    }
}

impl<'a, M: 'a> From<Thumb> for Element<'a, M> {
    fn from(t: Thumb) -> Self {
        Element::new(t)
    }
}

// ---------------------------------------------------------------------------------------------
// Seek bar

/// YouTube's scrubber: a 3 px track that grows to 5 px with a 13 px knob on hover, a time
/// tooltip over the hovered point, and a drag that previews the target and seeks once, on
/// release.
pub struct SeekBar<'a, M> {
    played: f32,
    buffered: f32,
    duration: f64,
    on_seek: Box<dyn Fn(f32) -> M + 'a>,
}

pub fn seek_bar<'a, M>(
    played: f32,
    buffered: f32,
    duration: f64,
    on_seek: impl Fn(f32) -> M + 'a,
) -> SeekBar<'a, M> {
    let clean = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    SeekBar {
        played: clean(played),
        buffered: clean(buffered),
        duration,
        on_seek: Box::new(on_seek),
    }
}

#[derive(Default)]
struct SeekState {
    dragging: Option<f32>,
    hover: Option<f32>,
    label: Option<(String, Paragraph)>,
}

impl<'a, M> Widget<M, Theme, Renderer> for SeekBar<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(16.0))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SeekState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(SeekState::default())
    }
    fn layout(&mut self, _tree: &mut Tree, _r: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, 16.0, Size::ZERO))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<SeekState>();
        let b = layout.bounds();
        let frac = |x: f32| ((x - b.x) / b.width.max(1.0)).clamp(0.0, 1.0);
        let set_label = |st: &mut SeekState, f: f32| {
            let text = nt_core::format::clock(f as f64 * self.duration);
            if st.label.as_ref().is_none_or(|(s, _)| *s != text) {
                let mut spec = text_spec(&text, f32::INFINITY, t::BODY_M);
                spec.wrapping = Wrapping::None;
                let para = Paragraph::with_text(spec);
                st.label = Some((text, para));
            }
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(b) {
                    let f = frac(p.x);
                    st.dragging = Some(f);
                    set_label(st, f);
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if st.dragging.is_some() {
                    let f = frac(position.x);
                    st.dragging = Some(f);
                    set_label(st, f);
                    shell.capture_event();
                    shell.request_redraw();
                } else {
                    let hover = cursor.position_over(b).map(|p| frac(p.x));
                    if hover != st.hover {
                        st.hover = hover;
                        if let Some(f) = hover {
                            set_label(st, f);
                        }
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if st.hover.take().is_some() {
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some(f) = st.dragging.take() {
                    shell.publish((self.on_seek)(f));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::text::Renderer as _;
        let st = tree.state.downcast_ref::<SeekState>();
        let b = layout.bounds();
        let active = st.hover.is_some() || st.dragging.is_some();
        let thick = if active { 5.0 } else { 3.0 };
        let y = b.y + b.height - 4.0 - thick / 2.0;
        let played = st.dragging.unwrap_or(self.played);
        let bar = |x0: f32, x1: f32| Rectangle {
            x: b.x + x0 * b.width,
            y,
            width: ((x1 - x0) * b.width).max(0.0),
            height: thick,
        };
        fill(renderer, bar(0.0, 1.0), 0.0, t::css_white(0.2));
        fill(
            renderer,
            bar(0.0, self.buffered.max(played)),
            0.0,
            t::css_white(0.4),
        );
        if let Some(h) = st.hover.filter(|_| st.dragging.is_none()) {
            fill(
                renderer,
                bar(played.min(h), played.max(h)),
                0.0,
                t::css_white(0.5),
            );
        }
        fill(renderer, bar(0.0, played), 0.0, t::RED);
        if active {
            let d = 13.0;
            fill(
                renderer,
                Rectangle {
                    x: b.x + played * b.width - d / 2.0,
                    y: y + thick / 2.0 - d / 2.0,
                    width: d,
                    height: d,
                },
                d / 2.0,
                t::RED,
            );
        }
        let at = st.dragging.or(st.hover);
        if let (Some(f), Some((_, para))) = (at, &st.label)
            && self.duration > 0.0
        {
            let sz = para.min_bounds();
            let w = sz.width + 16.0;
            let x = (b.x + f * b.width - w / 2.0).clamp(b.x, b.x + b.width - w);
            let r = Rectangle {
                x,
                y: b.y - sz.height - 16.0,
                width: w,
                height: sz.height + 8.0,
            };
            fill(renderer, r, 4.0, Color::from_rgba(0.1, 0.1, 0.1, 0.9));
            renderer.fill_paragraph(para, Point::new(r.x + 8.0, r.y + 4.0), t::TEXT, *viewport);
        }
    }
    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

impl<'a, M: 'a> From<SeekBar<'a, M>> for Element<'a, M> {
    fn from(s: SeekBar<'a, M>) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Auto-hiding player controls

/// The player surface: `base` (picture) always, `overlay` (controls) while the pointer moved
/// over it in the last 3 s, or always when `force` (paused, menu open). While hidden over the
/// picture the cursor hides too. Uses one redraw at the hide deadline, no polling.
pub struct AutoHide<'a, M> {
    base: Element<'a, M>,
    overlay: Element<'a, M>,
    force: bool,
}

pub fn auto_hide<'a, M>(
    base: impl Into<Element<'a, M>>,
    overlay: impl Into<Element<'a, M>>,
    force: bool,
) -> AutoHide<'a, M> {
    AutoHide {
        base: base.into(),
        overlay: overlay.into(),
        force,
    }
}

const HIDE_AFTER: Duration = Duration::from_secs(3);

#[derive(Default)]
struct HideState {
    last_move: Option<Instant>,
    shown: bool,
}

impl<'a, M> AutoHide<'a, M> {
    fn visible(&self, st: &HideState) -> bool {
        self.force || st.shown
    }
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for AutoHide<'a, M> {
    fn size(&self) -> Size<Length> {
        self.base.as_widget().size()
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HideState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(HideState::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![
            Tree::new(self.base.as_widget()),
            Tree::new(self.overlay.as_widget()),
        ]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.base.as_widget(), self.overlay.as_widget()]);
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let [bt, ot] = &mut tree.children[..] else {
            return layout::Node::new(Size::ZERO);
        };
        let base = self.base.as_widget_mut().layout(bt, renderer, limits);
        let size = base.size();
        let overlay = self.overlay.as_widget_mut().layout(
            ot,
            renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        layout::Node::with_children(size, vec![base, overlay])
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let st = tree.state.downcast_mut::<HideState>();
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if cursor.is_over(bounds) {
                    let now = Instant::now();
                    st.last_move = Some(now);
                    if !st.shown {
                        st.shown = true;
                        shell.request_redraw();
                    }
                    shell.request_redraw_at(now + HIDE_AFTER);
                } else if st.shown {
                    st.shown = false;
                    st.last_move = None;
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if st.shown {
                    st.shown = false;
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(now))
                if st.shown
                    && st
                        .last_move
                        .is_none_or(|t| now.saturating_duration_since(t) >= HIDE_AFTER) =>
            {
                st.shown = false;
            }
            _ => {}
        }
        let visible = self.visible(st);
        let mut children = layout.children();
        let (Some(bl), Some(ol)) = (children.next(), children.next()) else {
            return;
        };
        let [bt, ot] = &mut tree.children[..] else {
            return;
        };
        if visible {
            self.overlay
                .as_widget_mut()
                .update(ot, event, ol, cursor, renderer, clipboard, shell, viewport);
            if shell.is_event_captured() {
                return;
            }
        }
        self.base
            .as_widget_mut()
            .update(bt, event, bl, cursor, renderer, clipboard, shell, viewport);
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let st = tree.state.downcast_ref::<HideState>();
        let mut children = layout.children();
        let (Some(bl), Some(ol)) = (children.next(), children.next()) else {
            return;
        };
        let [bt, ot] = &tree.children[..] else {
            return;
        };
        self.base
            .as_widget()
            .draw(bt, renderer, theme, style, bl, cursor, viewport);
        if self.visible(st) {
            // A layer of its own: iced draws custom primitives (the video) after a layer's
            // quads, so the controls must sit in a later layer to be on top.
            renderer.with_layer(layout.bounds(), |renderer| {
                self.overlay
                    .as_widget()
                    .draw(ot, renderer, theme, style, ol, cursor, viewport);
            });
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let st = tree.state.downcast_ref::<HideState>();
        let mut children = layout.children();
        let (Some(bl), Some(ol)) = (children.next(), children.next()) else {
            return mouse::Interaction::None;
        };
        let [bt, ot] = &tree.children[..] else {
            return mouse::Interaction::None;
        };
        if !self.visible(st) {
            return if cursor.is_over(layout.bounds()) {
                mouse::Interaction::Hidden
            } else {
                mouse::Interaction::None
            };
        }
        let o = self
            .overlay
            .as_widget()
            .mouse_interaction(ot, ol, cursor, viewport, renderer);
        if o != mouse::Interaction::None {
            return o;
        }
        self.base
            .as_widget()
            .mouse_interaction(bt, bl, cursor, viewport, renderer)
    }
}

impl<'a, M: 'a> From<AutoHide<'a, M>> for Element<'a, M> {
    fn from(a: AutoHide<'a, M>) -> Self {
        Element::new(a)
    }
}

// ---------------------------------------------------------------------------------------------
// Centre bezel

/// The 52 px icon that flashes in the middle of the player after a click or a key, fading out
/// over 500 ms. Redraws only during the fade.
pub struct Bezel {
    icon: Option<(Icon, Instant)>,
}

pub fn bezel(icon: Option<(Icon, Instant)>) -> Bezel {
    Bezel { icon }
}

const BEZEL: Duration = Duration::from_millis(500);

#[derive(Default)]
struct BezelState {
    now: Option<Instant>,
}

impl<M> Widget<M, Theme, Renderer> for Bezel {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<BezelState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(BezelState::default())
    }
    fn layout(&mut self, _t: &mut Tree, _r: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        if let (Event::Window(window::Event::RedrawRequested(now)), Some((_, start))) =
            (event, self.icon)
        {
            tree.state.downcast_mut::<BezelState>().now = Some(*now);
            if now.saturating_duration_since(start) < BEZEL {
                shell.request_redraw();
            }
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let Some((icon, start)) = self.icon else {
            return;
        };
        let now = tree.state.downcast_ref::<BezelState>().now.unwrap_or(start);
        let t = now.saturating_duration_since(start).as_secs_f32() / BEZEL.as_secs_f32();
        if !(0.0..1.0).contains(&t) {
            return;
        }
        let alpha = 1.0 - t;
        let d = 52.0 * (1.0 + 0.4 * t);
        let c = layout.bounds().center();
        let r = Rectangle {
            x: c.x - d / 2.0,
            y: c.y - d / 2.0,
            width: d,
            height: d,
        };
        fill(renderer, r, d / 2.0, t::css_black(0.5 * alpha));
        let i = d * 0.6;
        draw_icon(
            renderer,
            icon,
            Rectangle {
                x: c.x - i / 2.0,
                y: c.y - i / 2.0,
                width: i,
                height: i,
            },
            Color {
                a: alpha,
                ..t::TEXT
            },
        );
    }
}

impl<'a, M: 'a> From<Bezel> for Element<'a, M> {
    fn from(b: Bezel) -> Self {
        Element::new(b)
    }
}

// ---------------------------------------------------------------------------------------------
// Chip bar

/// The topic chips, laid out in one row and scrolled horizontally by the horizontal wheel axis
/// (or Shift+wheel) and by dragging. Where the row is cut off it fades out over 50 px and shows
/// a 40 px round arrow button that pages it.
pub struct ChipBar<'a, M> {
    children: Vec<Element<'a, M>>,
    offset: &'a std::cell::Cell<f32>,
    gap: f32,
}

pub fn chip_bar<'a, M>(
    children: Vec<Element<'a, M>>,
    offset: &'a std::cell::Cell<f32>,
) -> ChipBar<'a, M> {
    ChipBar {
        children,
        offset,
        gap: 12.0,
    }
}

#[derive(Default)]
struct ChipState {
    content_w: f32,
    press: Option<(f32, f32)>,
    dragging: bool,
}

impl<'a, M> ChipBar<'a, M> {
    fn max(&self, st: &ChipState, width: f32) -> f32 {
        (st.content_w - width).max(0.0)
    }
    fn arrows(&self, st: &ChipState, b: Rectangle) -> (Option<Rectangle>, Option<Rectangle>) {
        let off = self.offset.get();
        let y = b.center_y() - 20.0;
        let left = (off > 0.5).then_some(Rectangle {
            x: b.x,
            y,
            width: 40.0,
            height: 40.0,
        });
        let right = (off < self.max(st, b.width) - 0.5).then_some(Rectangle {
            x: b.x + b.width - 40.0,
            y,
            width: 40.0,
            height: 40.0,
        });
        (left, right)
    }
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for ChipBar<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(t::CHIP_BAR))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ChipState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ChipState::default())
    }
    fn children(&self) -> Vec<Tree> {
        self.children
            .iter()
            .map(|c| Tree::new(c.as_widget()))
            .collect()
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = Size::new(limits.max().width, t::CHIP_BAR);
        let mut x = 0.0;
        let mut nodes = Vec::with_capacity(self.children.len());
        let child_limits = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, 32.0));
        for (child, ct) in self.children.iter_mut().zip(tree.children.iter_mut()) {
            let node = child.as_widget_mut().layout(ct, renderer, &child_limits);
            let w = node.size().width;
            nodes.push(node.move_to((x, (size.height - 32.0) / 2.0)));
            x += w + self.gap;
        }
        let st = tree.state.downcast_mut::<ChipState>();
        st.content_w = (x - self.gap).max(0.0);
        let max = self.max(st, size.width);
        self.offset.set(self.offset.get().clamp(0.0, max));
        layout::Node::with_children(size, nodes)
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let b = layout.bounds();
        let st = tree.state.downcast_mut::<ChipState>();
        let max = self.max(st, b.width);
        let set = |o: f32| self.offset.set(o.clamp(0.0, max));
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(b) => {
                let dx = match *delta {
                    mouse::ScrollDelta::Lines { x, .. } => -x * 40.0,
                    mouse::ScrollDelta::Pixels { x, .. } => -x,
                };
                if dx != 0.0 {
                    set(self.offset.get() + dx);
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(b) {
                    let (l, r) = self.arrows(st, b);
                    let page = (b.width - 120.0).max(80.0);
                    if l.is_some_and(|l| l.contains(p)) {
                        set(self.offset.get() - page);
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    if r.is_some_and(|r| r.contains(p)) {
                        set(self.offset.get() + page);
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    st.press = Some((p.x, self.offset.get()));
                    st.dragging = false;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((x0, o0)) = st.press
                    && (st.dragging || (position.x - x0).abs() > 6.0)
                {
                    st.dragging = true;
                    set(o0 - (position.x - x0));
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let dragged = st.dragging;
                st.press = None;
                st.dragging = false;
                if dragged {
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
        }
        let off = self.offset.get();
        let inner = Rectangle { x: b.x + off, ..b };
        let (l, r) = self.arrows(st, b);
        let over_arrow = cursor
            .position()
            .is_some_and(|p| l.is_some_and(|l| l.contains(p)) || r.is_some_and(|r| r.contains(p)));
        let child_cursor = if over_arrow || !cursor.is_over(b) {
            cursor.levitate()
        } else {
            cursor + Vector::new(off, 0.0)
        };
        for ((child, ct), cl) in self
            .children
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                ct,
                event,
                cl,
                child_cursor,
                renderer,
                clipboard,
                shell,
                &inner,
            );
        }
        let _ = viewport;
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let b = layout.bounds();
        let st = tree.state.downcast_ref::<ChipState>();
        let off = self.offset.get();
        let (l, r) = self.arrows(st, b);
        let over_arrow = cursor
            .position()
            .is_some_and(|p| l.is_some_and(|l| l.contains(p)) || r.is_some_and(|r| r.contains(p)));
        let child_cursor = if over_arrow || !cursor.is_over(b) {
            cursor.levitate()
        } else {
            cursor + Vector::new(off, 0.0)
        };
        let inner = Rectangle { x: b.x + off, ..b };
        renderer.with_layer(b, |renderer| {
            renderer.with_translation(Vector::new(-off, 0.0), |renderer| {
                for ((child, ct), cl) in self
                    .children
                    .iter()
                    .zip(tree.children.iter())
                    .zip(layout.children())
                {
                    if cl.bounds().intersects(&inner) {
                        child.as_widget().draw(
                            ct,
                            renderer,
                            theme,
                            style,
                            cl,
                            child_cursor,
                            &inner,
                        );
                    }
                }
            });
        });
        // Fades and arrows go in a layer above the chips' text.
        renderer.with_layer(b, |renderer| {
            let fade = |renderer: &mut Renderer, x: f32, to_right: bool| {
                let g = iced::gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
                    .add_stop(
                        0.0,
                        if to_right {
                            Color { a: 0.0, ..t::BG }
                        } else {
                            t::BG
                        },
                    )
                    .add_stop(
                        1.0,
                        if to_right {
                            t::BG
                        } else {
                            Color { a: 0.0, ..t::BG }
                        },
                    );
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle {
                            x,
                            y: b.y,
                            width: 90.0,
                            height: b.height,
                        },
                        ..Quad::default()
                    },
                    iced::Background::Gradient(iced::Gradient::Linear(g)),
                );
            };
            if let Some(l) = l {
                fade(renderer, b.x + 40.0, false);
                fill(
                    renderer,
                    Rectangle {
                        x: b.x,
                        y: b.y,
                        width: 40.0,
                        height: b.height,
                    },
                    0.0,
                    t::BG,
                );
                let hover = cursor.position().is_some_and(|p| l.contains(p));
                fill(renderer, l, 20.0, if hover { t::FILL } else { t::BG });
                draw_icon(
                    renderer,
                    Icon::ChevronLeft,
                    Rectangle {
                        x: l.x + 8.0,
                        y: l.y + 8.0,
                        width: 24.0,
                        height: 24.0,
                    },
                    t::TEXT,
                );
            }
            if let Some(r) = r {
                fade(renderer, b.x + b.width - 130.0, true);
                fill(
                    renderer,
                    Rectangle {
                        x: b.x + b.width - 40.0,
                        y: b.y,
                        width: 40.0,
                        height: b.height,
                    },
                    0.0,
                    t::BG,
                );
                let hover = cursor.position().is_some_and(|p| r.contains(p));
                fill(renderer, r, 20.0, if hover { t::FILL } else { t::BG });
                draw_icon(
                    renderer,
                    Icon::ChevronRight,
                    Rectangle {
                        x: r.x + 8.0,
                        y: r.y + 8.0,
                        width: 24.0,
                        height: 24.0,
                    },
                    t::TEXT,
                );
            }
        });
        let _ = viewport;
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let b = layout.bounds();
        let st = tree.state.downcast_ref::<ChipState>();
        let (l, r) = self.arrows(st, b);
        if cursor
            .position()
            .is_some_and(|p| l.is_some_and(|l| l.contains(p)) || r.is_some_and(|r| r.contains(p)))
        {
            return mouse::Interaction::Pointer;
        }
        if st.dragging {
            return mouse::Interaction::Grabbing;
        }
        let off = self.offset.get();
        let child_cursor = cursor + Vector::new(off, 0.0);
        let inner = Rectangle { x: b.x + off, ..b };
        self.children
            .iter()
            .zip(tree.children.iter())
            .zip(layout.children())
            .map(|((c, ct), cl)| {
                c.as_widget()
                    .mouse_interaction(ct, cl, child_cursor, &inner, renderer)
            })
            .max()
            .unwrap_or_default()
    }
}

impl<'a, M: 'a> From<ChipBar<'a, M>> for Element<'a, M> {
    fn from(c: ChipBar<'a, M>) -> Self {
        Element::new(c)
    }
}
// ---------------------------------------------------------------------------------------------
// Spinner

/// Eight dots going round, redrawn at 15 Hz while it is on screen.
pub struct Spinner {
    size: f32,
}

pub fn spinner(size: f32) -> Spinner {
    Spinner { size }
}

struct SpinState {
    start: Instant,
    now: Instant,
}

impl<M> Widget<M, Theme, Renderer> for Spinner {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SpinState>()
    }
    fn state(&self) -> tree::State {
        let now = Instant::now();
        tree::State::new(SpinState { start: now, now })
    }
    fn layout(&mut self, _tree: &mut Tree, _r: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(self.size, self.size, Size::new(self.size, self.size)))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event
            && layout.bounds().intersects(viewport)
        {
            let st = tree.state.downcast_mut::<SpinState>();
            st.now = *now;
            shell.request_redraw_at(*now + Duration::from_millis(66));
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let st = tree.state.downcast_ref::<SpinState>();
        let b = layout.bounds();
        let phase = (st.now.duration_since(st.start).as_secs_f32() * 1.25).fract();
        let c = b.center();
        let r = self.size / 2.0 - self.size / 10.0;
        let dot = self.size / 7.0;
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            let age = (phase - i as f32 / 8.0).rem_euclid(1.0);
            let alpha = 1.0 - 0.8 * age;
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x: c.x + r * a.cos() - dot / 2.0,
                        y: c.y + r * a.sin() - dot / 2.0,
                        width: dot,
                        height: dot,
                    },
                    border: Border {
                        radius: (dot / 2.0).into(),
                        ..Border::default()
                    },
                    ..Quad::default()
                },
                Color {
                    a: alpha,
                    ..t::TEXT_2
                },
            );
        }
    }
}

impl<'a, M: 'a> From<Spinner> for Element<'a, M> {
    fn from(s: Spinner) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Search field wrapper

/// Wraps the search `text_input`: reports focus changes, and while it is focused turns
/// Up/Down/Escape into suggestion-list messages before the input sees them.
pub struct SearchBox<'a, M> {
    inner: Element<'a, M>,
    focused: bool,
    on_focus: Box<dyn Fn(bool) -> M + 'a>,
    on_nav: Box<dyn Fn(i32) -> M + 'a>,
    on_escape: Option<M>,
    blur: u64,
}

pub fn search_box<'a, M>(
    inner: impl Into<Element<'a, M>>,
    focused: bool,
    on_focus: impl Fn(bool) -> M + 'a,
    on_nav: impl Fn(i32) -> M + 'a,
    on_escape: Option<M>,
) -> SearchBox<'a, M> {
    SearchBox {
        inner: inner.into(),
        focused,
        on_focus: Box::new(on_focus),
        on_nav: Box::new(on_nav),
        on_escape,
        blur: 0,
    }
}

impl<'a, M> SearchBox<'a, M> {
    /// Changing this token unfocuses the field on the next rebuild (iced 0.14 has no
    /// unfocus operation).
    pub fn blur(mut self, token: u64) -> Self {
        self.blur = token;
        self
    }
}

impl<'a, M: Clone + 'a> Widget<M, Theme, Renderer> for SearchBox<'a, M> {
    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<u64>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(self.blur)
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(self.inner.as_widget())]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.inner));
        let seen = tree.state.downcast_mut::<u64>();
        if *seen != self.blur {
            *seen = self.blur;
            if let Some(child) = tree.children.first_mut() {
                child
                    .state
                    .downcast_mut::<iced::widget::text_input::State<Paragraph>>()
                    .unfocus();
            }
        }
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let Some(child) = tree.children.first_mut() else {
            return layout::Node::new(Size::ZERO);
        };
        let node = self.inner.as_widget_mut().layout(child, renderer, limits);
        layout::Node::with_children(node.size(), vec![node])
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if let (Some(l), Some(child)) = (layout.children().next(), tree.children.first_mut()) {
            self.inner
                .as_widget_mut()
                .operate(child, l, renderer, operation);
        }
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let Some(inner_layout) = layout.children().next() else {
            return;
        };
        let is_focused = |tree: &Tree| {
            tree.children.first().is_some_and(|c| {
                c.state
                    .downcast_ref::<iced::widget::text_input::State<Paragraph>>()
                    .is_focused()
            })
        };
        if is_focused(tree)
            && let Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(k),
                ..
            }) = event
        {
            match k {
                Named::ArrowDown => {
                    shell.publish((self.on_nav)(1));
                    shell.capture_event();
                    return;
                }
                Named::ArrowUp => {
                    shell.publish((self.on_nav)(-1));
                    shell.capture_event();
                    return;
                }
                Named::Escape => {
                    if let Some(m) = &self.on_escape {
                        shell.publish(m.clone());
                        shell.capture_event();
                        return;
                    }
                }
                _ => {}
            }
        }
        let Some(child) = tree.children.first_mut() else {
            return;
        };
        self.inner.as_widget_mut().update(
            child,
            event,
            inner_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let now = is_focused(tree);
        if now != self.focused {
            self.focused = now;
            shell.publish((self.on_focus)(now));
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let (Some(l), Some(child)) = (layout.children().next(), tree.children.first()) {
            self.inner
                .as_widget()
                .draw(child, renderer, theme, style, l, cursor, viewport);
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        layout
            .children()
            .next()
            .zip(tree.children.first())
            .map(|(l, child)| {
                self.inner
                    .as_widget()
                    .mouse_interaction(child, l, cursor, viewport, renderer)
            })
            .unwrap_or_default()
    }
}

impl<'a, M: Clone + 'a> From<SearchBox<'a, M>> for Element<'a, M> {
    fn from(s: SearchBox<'a, M>) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Press hint

/// Passes everything through to `inner`, and publishes `message` when the left button goes
/// down over it, without capturing the press (so the button inside still gets its click).
pub struct PressHint<'a, M> {
    inner: Element<'a, M>,
    message: M,
}

pub fn press_hint<'a, M>(inner: impl Into<Element<'a, M>>, message: M) -> PressHint<'a, M> {
    PressHint {
        inner: inner.into(),
        message,
    }
}

impl<'a, M: Clone + 'a> Widget<M, Theme, Renderer> for PressHint<'a, M> {
    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.inner.as_widget().size_hint()
    }
    fn tag(&self) -> tree::Tag {
        self.inner.as_widget().tag()
    }
    fn state(&self) -> tree::State {
        self.inner.as_widget().state()
    }
    fn children(&self) -> Vec<Tree> {
        self.inner.as_widget().children()
    }
    fn diff(&self, tree: &mut Tree) {
        self.inner.as_widget().diff(tree)
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.inner.as_widget_mut().layout(tree, renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && cursor.is_over(layout.bounds())
            && !shell.is_event_captured()
        {
            shell.publish(self.message.clone());
        }
        self.inner.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.inner
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.inner
            .as_widget_mut()
            .operate(tree, layout, renderer, operation)
    }
}

impl<'a, M: Clone + 'a> From<PressHint<'a, M>> for Element<'a, M> {
    fn from(p: PressHint<'a, M>) -> Self {
        Element::new(p)
    }
}
