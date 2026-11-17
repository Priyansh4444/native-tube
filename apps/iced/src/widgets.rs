//! Small custom widgets iced doesn't ship: text clamped to N lines with an ellipsis, a
//! thumbnail with its duration badge, a spinner, a seek bar, and a wrapper that reports the
//! search field's focus and steals Up/Down/Escape for the suggestion list.

use crate::theme as t;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self as atext, Paragraph as _};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse};
use iced::alignment;
use iced::keyboard::{self, key::Named};
use iced::widget::image::Handle;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{
    Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Renderer, Size, Theme,
    window,
};
use std::time::{Duration, Instant};

type Paragraph = <Renderer as atext::Renderer>::Paragraph;

fn text_spec(content: &str, width: f32, size: f32, lh: f32, font: Font) -> atext::Text<&str> {
    atext::Text {
        content,
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(size),
        line_height: LineHeight::Absolute(Pixels(lh)),
        font,
        align_x: atext::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::WordOrGlyph,
    }
}

// ---------------------------------------------------------------------------------------------
// Clamped text

/// Text that wraps to at most `lines` lines, cutting the rest with "…".
pub struct Clamp<'a> {
    content: std::borrow::Cow<'a, str>,
    size: f32,
    line_height: f32,
    lines: usize,
    color: Option<Color>,
    font: Font,
    /// Reserve the full `lines` height even when the text is shorter.
    reserve: bool,
}

pub fn clamp<'a>(
    content: impl Into<std::borrow::Cow<'a, str>>,
    size: f32,
    line_height: f32,
    lines: usize,
) -> Clamp<'a> {
    Clamp {
        content: content.into(),
        size,
        line_height,
        lines,
        color: None,
        font: Font::DEFAULT,
        reserve: false,
    }
}

impl<'a> Clamp<'a> {
    /// Without a colour the text takes the enclosing style's (so links can change on hover).
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn font(mut self, f: Font) -> Self {
        self.font = f;
        self
    }
    pub fn reserve(mut self) -> Self {
        self.reserve = true;
        self
    }
}

struct ClampState {
    content: String,
    width: f32,
    lines: usize,
    para: Option<Paragraph>,
}

impl<'a, M> Widget<M, Theme, Renderer> for Clamp<'a> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ClampState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ClampState {
            content: String::new(),
            width: -1.0,
            lines: 0,
            para: None,
        })
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let st = tree.state.downcast_mut::<ClampState>();
        if st.para.is_none()
            || st.width != width
            || st.content != *self.content
            || st.lines != self.lines
        {
            st.content.clear();
            st.content.push_str(&self.content);
            st.width = width;
            st.lines = self.lines;
            st.para = Some(fit(
                &self.content,
                width,
                self.size,
                self.line_height,
                self.lines,
                self.font,
            ));
        }
        let h = st
            .para
            .as_ref()
            .map(|p| p.min_bounds().height)
            .unwrap_or(0.0);
        let h = if self.reserve {
            self.line_height * self.lines as f32
        } else {
            h
        };
        layout::Node::new(Size::new(width, h))
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

/// Shape `content` into at most `lines` lines; if it overflows, cut it at the longest prefix
/// that still fits with an ellipsis appended (binary search over character boundaries).
fn fit(content: &str, width: f32, size: f32, lh: f32, lines: usize, font: Font) -> Paragraph {
    let full = Paragraph::with_text(text_spec(content, width, size, lh, font));
    let max_h = lh * lines as f32 + 0.5;
    if full.min_bounds().height <= max_h || content.is_empty() {
        return full;
    }
    let mut buf = String::with_capacity(content.len().min(4096) + 4);
    let attempt = |cut: usize, buf: &mut String| {
        buf.clear();
        buf.push_str(content[..cut].trim_end());
        buf.push('…');
        let p = Paragraph::with_text(text_spec(buf, width, size, lh, font));
        (p.min_bounds().height <= max_h).then_some(p)
    };
    // Binary search for the longest prefix that fits. No glyph is narrower than ~0.2 em, so
    // nothing past that many characters per line can fit; a short range keeps this to a few
    // shaping passes.
    let max_chars = (lines as f32 * width / (size * 0.2)).ceil() as usize + 1;
    let bound = content
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let (mut lo, mut hi) = (0usize, bound);
    let mut best = None;
    while lo < hi {
        let mid = floor_char(content, (lo + hi).div_ceil(2));
        if mid <= lo {
            break;
        }
        match attempt(mid, &mut buf) {
            Some(p) => {
                best = Some(p);
                lo = mid;
            }
            None => hi = mid - 1,
        }
    }
    best.unwrap_or_else(|| Paragraph::with_text(text_spec("…", width, size, lh, font)))
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl<'a, M: 'a> From<Clamp<'a>> for Element<'a, M> {
    fn from(c: Clamp<'a>) -> Self {
        Element::new(c)
    }
}

// ---------------------------------------------------------------------------------------------
// Thumbnail

/// A fixed-size image with a raised placeholder, rounded corners and an optional badge. The
/// badge background is drawn as a stretched 1x1 image so it lands above the thumbnail within
/// the same layer (iced draws a layer's quads before its images and its text after them).
pub struct Thumb {
    handle: Option<Handle>,
    width: f32,
    height: f32,
    radius: f32,
    badge: Option<(String, bool)>,
    cover: bool,
}

pub fn thumb(handle: Option<Handle>, width: f32, height: f32, radius: f32) -> Thumb {
    Thumb {
        handle,
        width,
        height,
        radius,
        badge: None,
        cover: false,
    }
}

impl Thumb {
    /// `live` paints the badge red.
    pub fn badge(mut self, text: Option<String>, live: bool) -> Self {
        self.badge = text.map(|t| (t, live));
        self
    }
    /// Crop to fill (for banners) instead of stretching.
    pub fn cover(mut self) -> Self {
        self.cover = true;
        self
    }
}

struct ThumbState {
    badge: Option<(String, Paragraph)>,
}

impl<M> Widget<M, Theme, Renderer> for Thumb {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width), Length::Fixed(self.height))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ThumbState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ThumbState { badge: None })
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let st = tree.state.downcast_mut::<ThumbState>();
        match &self.badge {
            Some((text, _)) => {
                if st.badge.as_ref().is_none_or(|(s, _)| s != text) {
                    let mut spec = text_spec(text, f32::INFINITY, 12.0, 16.0, t::MEDIUM);
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
        use iced::advanced::Renderer as _;
        use iced::advanced::image::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let b = layout.bounds();
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
            None => renderer.fill_quad(
                Quad {
                    bounds: b,
                    border: Border {
                        radius: self.radius.into(),
                        ..Border::default()
                    },
                    ..Quad::default()
                },
                t::RAISED,
            ),
        }
        let st = tree.state.downcast_ref::<ThumbState>();
        if let (Some((_, live)), Some((_, para))) = (&self.badge, &st.badge) {
            let sz = para.min_bounds();
            let (pw, ph) = (6.0, 1.0);
            let r = Rectangle {
                x: b.x + b.width - 8.0 - sz.width - 2.0 * pw,
                y: b.y + b.height - 8.0 - sz.height - 2.0 * ph,
                width: sz.width + 2.0 * pw,
                height: sz.height + 2.0 * ph,
            };
            let mut img = iced::advanced::image::Image::new(if *live {
                t::red_pixel()
            } else {
                t::badge_pixel()
            });
            img.border_radius = 4.0.into();
            renderer.draw_image(img, r, r);
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
// Seek bar

/// Played / buffered / remaining track. Press and drag to seek; reports a fraction 0..1.
pub struct SeekBar<'a, M> {
    played: f32,
    buffered: f32,
    on_seek: Box<dyn Fn(f32) -> M + 'a>,
}

pub fn seek_bar<'a, M>(
    played: f32,
    buffered: f32,
    on_seek: impl Fn(f32) -> M + 'a,
) -> SeekBar<'a, M> {
    SeekBar {
        played: played.clamp(0.0, 1.0),
        buffered: buffered.clamp(0.0, 1.0),
        on_seek: Box::new(on_seek),
    }
}

#[derive(Default)]
struct SeekState {
    dragging: Option<f32>,
    hovered: bool,
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
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(b) {
                    let f = frac(p.x);
                    st.dragging = Some(f);
                    shell.publish((self.on_seek)(f));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let hovered = cursor.is_over(b);
                if hovered != st.hovered {
                    st.hovered = hovered;
                    shell.request_redraw();
                }
                if st.dragging.is_some() {
                    let f = frac(position.x);
                    st.dragging = Some(f);
                    shell.publish((self.on_seek)(f));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if st.dragging.is_some() =>
            {
                st.dragging = None;
                shell.capture_event();
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
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let st = tree.state.downcast_ref::<SeekState>();
        let b = layout.bounds();
        let thick = if st.hovered || st.dragging.is_some() {
            5.0
        } else {
            3.0
        };
        let y = b.center_y() - thick / 2.0;
        let played = st.dragging.unwrap_or(self.played);
        let bar = |x0: f32, x1: f32| Rectangle {
            x: b.x + x0 * b.width,
            y,
            width: ((x1 - x0) * b.width).max(0.0),
            height: thick,
        };
        let q = |r: Rectangle| Quad {
            bounds: r,
            border: Border {
                radius: (thick / 2.0).into(),
                ..Border::default()
            },
            ..Quad::default()
        };
        renderer.fill_quad(q(bar(0.0, 1.0)), Color::from_rgba(1.0, 1.0, 1.0, 0.25));
        renderer.fill_quad(
            q(bar(0.0, self.buffered.max(played))),
            Color::from_rgba(1.0, 1.0, 1.0, 0.45),
        );
        renderer.fill_quad(q(bar(0.0, played)), t::ACCENT);
        if st.hovered || st.dragging.is_some() {
            let d = 13.0;
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x: b.x + played * b.width - d / 2.0,
                        y: b.center_y() - d / 2.0,
                        width: d,
                        height: d,
                    },
                    border: Border {
                        radius: (d / 2.0).into(),
                        ..Border::default()
                    },
                    ..Quad::default()
                },
                t::ACCENT,
            );
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
            tree.children[0]
                .state
                .downcast_mut::<iced::widget::text_input::State<Paragraph>>()
                .unfocus();
        }
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self
            .inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(node.size(), vec![node])
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if let Some(l) = layout.children().next() {
            self.inner
                .as_widget_mut()
                .operate(&mut tree.children[0], l, renderer, operation);
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
            tree.children[0]
                .state
                .downcast_ref::<iced::widget::text_input::State<Paragraph>>()
                .is_focused()
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
        self.inner.as_widget_mut().update(
            &mut tree.children[0],
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
        if let Some(l) = layout.children().next() {
            self.inner.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                l,
                cursor,
                viewport,
            );
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
            .map(|l| {
                self.inner.as_widget().mouse_interaction(
                    &tree.children[0],
                    l,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .unwrap_or_default()
    }
}

impl<'a, M: Clone + 'a> From<SearchBox<'a, M>> for Element<'a, M> {
    fn from(s: SearchBox<'a, M>) -> Self {
        Element::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_long_description() {
        let s = "High-intensity ⚡ code tutorials and tech news to help you ship your app faster. New videos every week covering the topics every programmer should know. \n\nThe original home of #100SecondsOfCode #TheCodeReport and #CodeThisNotThat. Created by Jeff Delaney. \n\nWant more Fireship?\n\n🗞 Newsletter";
        let full = Paragraph::with_text(text_spec(s, 900.0, 14.0, 20.0, Font::DEFAULT));
        eprintln!("full h={}", full.min_bounds().height);
        let p = fit(s, 1000.0, 14.0, 20.0, 2, Font::DEFAULT);
        eprintln!("fit h={} w={}", p.min_bounds().height, p.min_bounds().width);
        assert!(p.min_bounds().width > 200.0);
    }
}
