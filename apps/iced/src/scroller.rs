//! The page scroller: a custom iced widget driven by `nt_core::scroll::ScrollAnimator`.
//!
//! iced's own `scrollable` is not used anywhere on the main feed. This widget takes the raw
//! `WheelScrolled` events (wheel notches arrive as `ScrollDelta::Lines`, touchpad deltas as
//! `ScrollDelta::Pixels`, finger lift as a zero `Pixels` delta), feeds them to the animator,
//! ticks it once per presented frame on `RedrawRequested`, and draws its children translated
//! by `-offset`. While the offset moves inside the rows that are already built, a frame costs a
//! translation and a draw; no message, no `view`, no layout. Only when the offset leaves the
//! middle of the built band (or nears the end) does it publish a message so the app rebuilds
//! the band around the new offset.
//!
//! Content is a set of vertical "lanes" (one for most pages, two for the wide watch page).
//! Each lane stacks its items from `top`; the app passes the height it assumed for every
//! built item, and the widget records the measured height in the model's cache so later
//! builds place items exactly. Children are diffed by key, so recycled rows keep their
//! widget state (shaped text) when the band moves.

use crate::clock;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse};
use iced::keyboard::{self, key::Named};
use iced::{
    Border, Color, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector, window,
};
use nt_core::scroll::{self, ScrollAnimator};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

/// How long the scrollbar stays up after the last motion.
const SCROLLBAR_LINGER: f64 = 0.9;
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_HIT_W: f32 = 14.0;

/// Round an offset to whole physical pixels, so text doesn't shimmer during slow scrolls.
fn snap(offset: f32, scale: f32) -> f32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (offset * scale).round() / scale
}

thread_local! {
    /// The last cursor position in window coordinates (menus anchor to it).
    pub static LAST_CURSOR: Cell<iced::Point> = const { Cell::new(iced::Point::ORIGIN) };
}

/// Everything a page keeps about its scroll position. Lives in the page, outside the widget
/// tree, so Back restores it exactly.
pub struct ScrollModel {
    pub anim: RefCell<ScrollAnimator>,
    /// The offset drawn by the last frame.
    offset: Cell<f32>,
    /// The offset the current band of rows was built for.
    built_at: Cell<f32>,
    viewport: Cell<Size>,
    content_h: Cell<f32>,
    heights: RefCell<HashMap<u64, f32>>,
    heights_width: Cell<f32>,
    /// Bumped whenever a measured height changes, so views can cache prefix sums.
    heights_version: Cell<u64>,
    last_motion: Cell<f64>,
    near_end_sent: Cell<bool>,
    drag: Cell<Option<f32>>,
    hover_bar: Cell<bool>,
}

impl Default for ScrollModel {
    fn default() -> Self {
        ScrollModel {
            anim: RefCell::new(ScrollAnimator::new()),
            offset: Cell::new(0.0),
            built_at: Cell::new(0.0),
            viewport: Cell::new(Size::new(1208.0, 744.0)),
            content_h: Cell::new(0.0),
            heights: RefCell::new(HashMap::new()),
            heights_width: Cell::new(0.0),
            heights_version: Cell::new(1),
            last_motion: Cell::new(f64::NEG_INFINITY),
            near_end_sent: Cell::new(false),
            drag: Cell::new(None),
            hover_bar: Cell::new(false),
        }
    }
}

impl ScrollModel {
    pub fn offset(&self) -> f32 {
        self.offset.get()
    }
    pub fn viewport(&self) -> Size {
        self.viewport.get()
    }
    /// Called by `view` when it builds the band of rows for the current offset.
    pub fn mark_built(&self) -> f32 {
        let o = self.offset.get();
        self.built_at.set(o);
        o
    }
    /// The measured height of an item, if it has been laid out at the current width.
    pub fn height(&self, key: u64) -> Option<f32> {
        self.heights.borrow().get(&key).copied()
    }
    pub fn heights_version(&self) -> u64 {
        self.heights_version.get()
    }
    /// Lets the next near-end check fire again (after a page was appended or a load failed).
    pub fn rearm_near_end(&self) {
        self.near_end_sent.set(false);
    }
    pub fn near_end(&self) -> bool {
        let a = self.anim.borrow();
        let vh = self.viewport.get().height as f64;
        a.max_offset() - a.offset().max(a.target()) <= 1.5 * vh
    }
    /// Feed helpers used by the autopilot and keyboard shortcuts coming from `update`.
    pub fn with<R>(&self, f: impl FnOnce(&mut ScrollAnimator, f64) -> R) -> R {
        f(&mut self.anim.borrow_mut(), clock::now())
    }
}

/// One item of a lane.
pub struct Item<'a, M> {
    pub key: u64,
    /// The height the app used to place this item (measured if known, else an estimate).
    pub assumed: f32,
    pub element: Element<'a, M>,
}

pub struct Lane<'a, M> {
    pub x: f32,
    pub width: f32,
    /// Where the first built item starts.
    pub top: f32,
    /// The lane's whole height as the app computed it from assumed heights.
    pub assumed_total: f32,
    pub items: Vec<Item<'a, M>>,
}

pub type KeyHandler<'a, M> = Box<dyn Fn(&keyboard::Key, keyboard::Modifiers) -> Option<M> + 'a>;

pub struct Scroller<'a, M> {
    model: &'a ScrollModel,
    lanes: Vec<(f32, f32, f32, f32, usize)>, // x, width, top, assumed_total, item count
    keys: Vec<u64>,
    assumed: Vec<f32>,
    children: Vec<Element<'a, M>>,
    on_move: M,
    on_key: Option<KeyHandler<'a, M>>,
    keys_enabled: bool,
    scale: f32,
    bottom: f32,
}

impl<'a, M: Clone + 'a> Scroller<'a, M> {
    pub fn new(model: &'a ScrollModel, lanes: Vec<Lane<'a, M>>, on_move: M) -> Self {
        let mut s = Scroller {
            model,
            lanes: Vec::with_capacity(lanes.len()),
            keys: Vec::new(),
            assumed: Vec::new(),
            children: Vec::new(),
            on_move,
            on_key: None,
            keys_enabled: true,
            scale: 1.0,
            bottom: 24.0,
        };
        for lane in lanes {
            s.lanes.push((
                lane.x,
                lane.width,
                lane.top,
                lane.assumed_total,
                lane.items.len(),
            ));
            for item in lane.items {
                s.keys.push(item.key);
                s.assumed.push(item.assumed);
                s.children.push(item.element);
            }
        }
        s
    }
    pub fn on_key(mut self, f: Option<KeyHandler<'a, M>>) -> Self {
        self.on_key = f;
        self
    }
    pub fn keys_enabled(mut self, on: bool) -> Self {
        self.keys_enabled = on;
        self
    }
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    fn thumb(&self, bounds: Rectangle) -> Option<Rectangle> {
        let content = self.model.content_h.get();
        let vh = bounds.height;
        if content <= vh + 0.5 {
            return None;
        }
        let max = (content - vh).max(1.0);
        let h = (vh * vh / content).max(32.0).min(vh);
        let y = bounds.y + (self.model.offset.get() / max).clamp(0.0, 1.0) * (vh - h);
        Some(Rectangle {
            x: bounds.x + bounds.width - SCROLLBAR_W - 3.0,
            y,
            width: SCROLLBAR_W,
            height: h,
        })
    }

    fn content_cursor(&self, cursor: mouse::Cursor, bounds: Rectangle) -> mouse::Cursor {
        let t = Vector::new(0.0, self.model.offset.get());
        match cursor.position_over(bounds) {
            Some(p) if self.model.drag.get().is_none() => mouse::Cursor::Available(p + t),
            _ => cursor.levitate() + t,
        }
    }

    /// After any change of offset: does the band need rebuilding, or is it time to load more?
    fn check_band(&self, shell: &mut Shell<'_, M>) {
        let vh = self.model.viewport.get().height;
        let moved = (self.model.offset.get() - self.model.built_at.get()).abs() > 0.5 * vh;
        let near = self.model.near_end() && !self.model.near_end_sent.get();
        if near {
            self.model.near_end_sent.set(true);
        }
        if moved || near {
            shell.publish(self.on_move.clone());
        }
    }

    fn after_input(&self, shell: &mut Shell<'_, M>) {
        self.model.last_motion.set(clock::now());
        shell.request_redraw();
    }
}

#[derive(Default)]
struct KeyState {
    keys: Vec<u64>,
}

impl<'a, M: Clone + 'a> Widget<M, Theme, Renderer> for Scroller<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyState {
            keys: self.keys.clone(),
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.children
            .iter()
            .map(|c| Tree::new(c.as_widget()))
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<KeyState>();
        if state.keys == self.keys {
            for (t, c) in tree.children.iter_mut().zip(&self.children) {
                t.diff(c.as_widget());
            }
            return;
        }
        let mut old: HashMap<u64, Tree> =
            state.keys.drain(..).zip(tree.children.drain(..)).collect();
        for (key, child) in self.keys.iter().zip(&self.children) {
            let t = match old.remove(key) {
                Some(mut t) => {
                    t.diff(child.as_widget());
                    t
                }
                None => Tree::new(child.as_widget()),
            };
            tree.children.push(t);
        }
        state.keys.clone_from(&self.keys);
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.max();
        let width_key: f32 = self.lanes.iter().map(|l| l.1).sum();
        if self.model.heights_width.get() != width_key {
            self.model.heights.borrow_mut().clear();
            self.model.heights_width.set(width_key);
        }
        let mut nodes = Vec::with_capacity(self.children.len());
        let mut content_h: f32 = 0.0;
        let mut heights = self.model.heights.borrow_mut();
        let mut changed = false;
        let mut items = self
            .children
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(self.keys.iter().zip(self.assumed.iter()));
        for &(x, width, top, assumed_total, count) in &self.lanes {
            let mut y = top;
            let mut total = assumed_total;
            let item_limits =
                layout::Limits::new(Size::new(width, 0.0), Size::new(width, f32::INFINITY));
            for ((child, child_tree), (key, assumed)) in items.by_ref().take(count) {
                let node = child
                    .as_widget_mut()
                    .layout(child_tree, renderer, &item_limits);
                let h = node.size().height;
                if heights.insert(*key, h) != Some(h) {
                    changed = true;
                }
                total += h - assumed;
                nodes.push(node.move_to((x, y)));
                y += h;
            }
            content_h = content_h.max(total.max(y));
        }
        if changed {
            self.model
                .heights_version
                .set(self.model.heights_version.get().wrapping_add(1));
        }
        drop(heights);
        let content_h = content_h + self.bottom;
        self.model.viewport.set(size);
        self.model.content_h.set(content_h);
        let mut a = self.model.anim.borrow_mut();
        a.set_viewport(size.height as f64);
        a.set_max_offset((content_h - size.height) as f64);
        self.model.offset.set(snap(a.offset() as f32, self.scale));
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
        let bounds = layout.bounds();
        let model = self.model;

        if let Event::Window(window::Event::RedrawRequested(at)) = event {
            let now = clock::secs(*at);
            let mut a = model.anim.borrow_mut();
            if a.needs_frame() {
                let off = snap(a.tick(now) as f32, self.scale);
                if off != model.offset.get() {
                    model.offset.set(off);
                    model.last_motion.set(now);
                }
                if a.needs_frame() {
                    shell.request_redraw();
                }
            }
            drop(a);
            // Keep the scrollbar up a moment after motion, then one frame to hide it.
            let since = now - model.last_motion.get();
            if since < SCROLLBAR_LINGER && !model.anim.borrow().needs_frame() {
                shell.request_redraw_at(
                    *at + Duration::from_secs_f64(SCROLLBAR_LINGER - since + 0.01),
                );
            }
            self.check_band(shell);
        }

        // Scrollbar drag goes first: it owns the pointer while held.
        if let Some(grab) = model.drag.get() {
            match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    if let Some(thumb) = self.thumb(bounds) {
                        let track = bounds.height - thumb.height;
                        let max = model.content_h.get() - bounds.height;
                        let frac =
                            ((position.y - grab - bounds.y) / track.max(1.0)).clamp(0.0, 1.0);
                        model.anim.borrow_mut().jump_to((frac * max) as f64);
                        self.after_input(shell);
                    }
                    shell.capture_event();
                    return;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    model.drag.set(None);
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
                _ => {}
            }
        }

        if let Event::Mouse(mouse::Event::CursorMoved { .. }) = event {
            let over = cursor
                .position_over(bounds)
                .is_some_and(|p| p.x >= bounds.x + bounds.width - SCROLLBAR_HIT_W);
            if over != model.hover_bar.get() {
                model.hover_bar.set(over);
                shell.request_redraw();
            }
        }

        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let Some(p) = cursor.position_over(bounds)
        {
            if p.x >= bounds.x + bounds.width - SCROLLBAR_HIT_W
                && let Some(thumb) = self.thumb(bounds)
            {
                let grab = if p.y >= thumb.y && p.y <= thumb.y + thumb.height {
                    p.y - thumb.y
                } else {
                    // Clicking the track centres the thumb under the pointer, then drags.
                    let grab = thumb.height / 2.0;
                    let track = bounds.height - thumb.height;
                    let max = model.content_h.get() - bounds.height;
                    let frac = ((p.y - grab - bounds.y) / track.max(1.0)).clamp(0.0, 1.0);
                    model.anim.borrow_mut().jump_to((frac * max) as f64);
                    grab
                };
                model.drag.set(Some(grab));
                self.after_input(shell);
                shell.capture_event();
                return;
            }
            // A click on the content stops a fling where it is.
            model.anim.borrow_mut().cancel_fling(clock::now());
            shell.request_redraw();
        }

        // Children, in content coordinates.
        let content_cursor = self.content_cursor(cursor, bounds);
        let off = model.offset.get();
        let content_viewport = Rectangle {
            y: bounds.y + off,
            ..bounds
        };
        let _ = viewport;
        for ((child, tree), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree,
                event,
                layout,
                content_cursor,
                renderer,
                clipboard,
                shell,
                &content_viewport,
            );
        }
        if shell.is_event_captured() {
            return;
        }

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.position().is_some() => {
                let now = clock::now();
                let mut a = model.anim.borrow_mut();
                match *delta {
                    // winit: +y is "away from the user" (up); the animator's + is down.
                    mouse::ScrollDelta::Lines { y, .. } => {
                        if y != 0.0 {
                            a.wheel_ticks(-y as f64, now);
                        }
                    }
                    mouse::ScrollDelta::Pixels { x, y } => {
                        if x == 0.0 && y == 0.0 {
                            // winit sends an empty pixel delta for wl_pointer.axis_stop.
                            a.fling_start(now);
                        } else if y != 0.0 {
                            // winit hands over -axis * scale; undo both, then Chrome's x12.
                            let raw = -(y / self.scale) as f64;
                            a.wheel_pixels(scroll::wayland_axis_to_px(raw), now);
                        }
                    }
                }
                drop(a);
                self.after_input(shell);
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if self.keys_enabled =>
            {
                if let Some(f) = &self.on_key
                    && let Some(msg) = f(key, *modifiers)
                {
                    model.anim.borrow_mut().cancel_fling(clock::now());
                    shell.publish(msg);
                    shell.capture_event();
                    return;
                }
                if modifiers.alt() || modifiers.control() || modifiers.logo() {
                    return;
                }
                let now = clock::now();
                let vh = bounds.height as f64;
                let mut a = model.anim.borrow_mut();
                let handled = match key {
                    keyboard::Key::Named(Named::ArrowDown) => {
                        let s = a.line_step();
                        a.scroll_by(s, now);
                        true
                    }
                    keyboard::Key::Named(Named::ArrowUp) => {
                        let s = a.line_step();
                        a.scroll_by(-s, now);
                        true
                    }
                    keyboard::Key::Named(Named::PageDown) => {
                        let s = a.page_step(vh);
                        a.scroll_by(s, now);
                        true
                    }
                    keyboard::Key::Named(Named::PageUp) => {
                        let s = a.page_step(vh);
                        a.scroll_by(-s, now);
                        true
                    }
                    keyboard::Key::Named(Named::Space) => {
                        let s = a.page_step(vh);
                        a.scroll_by(if modifiers.shift() { -s } else { s }, now);
                        true
                    }
                    keyboard::Key::Named(Named::Home) => {
                        let s = a.document_step();
                        a.scroll_by(-s, now);
                        true
                    }
                    keyboard::Key::Named(Named::End) => {
                        let s = a.document_step();
                        a.scroll_by(s, now);
                        true
                    }
                    _ => false,
                };
                drop(a);
                if handled {
                    self.after_input(shell);
                    shell.capture_event();
                }
            }
            _ => {}
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
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let Some(visible) = bounds.intersection(viewport) else {
            return;
        };
        let off = self.model.offset.get();
        let content_cursor = self.content_cursor(cursor, bounds);
        let content_viewport = Rectangle {
            y: visible.y + off,
            ..visible
        };
        renderer.with_layer(visible, |renderer| {
            renderer.with_translation(Vector::new(0.0, -off), |renderer| {
                for ((child, tree), layout) in self
                    .children
                    .iter()
                    .zip(&tree.children)
                    .zip(layout.children())
                {
                    if layout.bounds().intersects(&content_viewport) {
                        child.as_widget().draw(
                            tree,
                            renderer,
                            theme,
                            style,
                            layout,
                            content_cursor,
                            &content_viewport,
                        );
                    }
                }
            });
        });

        let show = clock::now() - self.model.last_motion.get() < SCROLLBAR_LINGER
            || self.model.drag.get().is_some()
            || self.model.hover_bar.get()
            || self.model.anim.borrow().is_animating();
        if show && let Some(thumb) = self.thumb(bounds) {
            renderer.with_layer(visible, |renderer| {
                renderer.fill_quad(
                    Quad {
                        bounds: thumb,
                        border: Border {
                            radius: (SCROLLBAR_W / 2.0).into(),
                            ..Border::default()
                        },
                        ..Quad::default()
                    },
                    Color::from_rgba8(
                        0xaa,
                        0xaa,
                        0xaa,
                        if self.model.drag.get().is_some() {
                            0.9
                        } else {
                            0.6
                        },
                    ),
                );
            });
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if self.model.drag.get().is_some() {
            return mouse::Interaction::Grabbing;
        }
        let content_cursor = self.content_cursor(cursor, bounds);
        let off = self.model.offset.get();
        let content_viewport = Rectangle {
            y: bounds.y + off,
            ..bounds
        };
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, l)| l.bounds().intersects(&content_viewport))
            .map(|((c, t), l)| {
                c.as_widget()
                    .mouse_interaction(t, l, content_cursor, &content_viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }
}

impl<'a, M: Clone + 'a> From<Scroller<'a, M>> for Element<'a, M> {
    fn from(s: Scroller<'a, M>) -> Self {
        Element::new(s)
    }
}

/// The root wrapper: counts presented frames and emits pending marks from `draw`, which iced
/// calls exactly once per `RedrawRequested`, right before `present`.
pub struct FrameHook<'a, M> {
    inner: Element<'a, M>,
}

thread_local! {
    static PENDING_MARKS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// With `NT_TRACE` set, print every occurrence of an event (the core prints only the first of
/// each mark), so repeated opens can be timed from the log.
pub fn trace_event(name: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("NT_TRACE").is_some()) {
        eprintln!("NT_EVENT {name} {:.1}", nt_core::metrics::now() * 1000.0);
    }
}

/// Emit `name` with the next presented frame.
pub fn mark_on_next_frame(name: &'static str) {
    PENDING_MARKS.with(|p| {
        let mut p = p.borrow_mut();
        if !p.contains(&name) {
            p.push(name);
        }
    });
}

pub fn frame_hook<'a, M: 'a>(inner: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(FrameHook {
        inner: inner.into(),
    })
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for FrameHook<'a, M> {
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
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            LAST_CURSOR.with(|c| c.set(*position));
        }
        self.inner.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
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
        nt_core::metrics::frame();
        PENDING_MARKS.with(|p| {
            for name in p.borrow_mut().drain(..) {
                nt_core::metrics::mark(name);
                trace_event(name);
            }
        });
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
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<iced::advanced::overlay::Element<'b, M, Theme, Renderer>> {
        self.inner
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}
