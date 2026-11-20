//! Layout-aware targets for the probe and focusable iced controls. Iced buttons
//! only handle pointer activation, so keyboard activation is owned by this wrapper.
use crate::app::{Message, PreviewTarget};
use crate::theme::TEXT;
use crate::view::El;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Operation, Tree, Widget, operation::Focusable, tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, mouse};
use iced::keyboard::{self, key::Named};
use iced::{Border, Color, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, window};
use std::borrow::Cow;
use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    static KEYBOARD: Cell<bool> = const { Cell::new(false) };
}

pub fn input_mode(event: &Event) {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(_)) => KEYBOARD.with(|k| k.set(false)),
        Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
            if matches!(
                key.as_ref(),
                keyboard::Key::Named(Named::Tab) | keyboard::Key::Character("/")
            ) =>
        {
            KEYBOARD.with(|k| k.set(true));
        }
        _ => {}
    }
}

pub fn keyboard_mode() -> bool {
    KEYBOARD.with(Cell::get)
}

pub fn border(renderer: &mut Renderer, bounds: Rectangle) {
    renderer.fill_quad(
        Quad {
            bounds: bounds.expand(4),
            border: Border {
                color: TEXT,
                width: 2.0,
                radius: 8.0.into(),
            },
            ..Quad::default()
        },
        Color::TRANSPARENT,
    );
}

#[derive(Default)]
struct Focus {
    keyboard: bool,
    dwell: Option<(Point, Instant)>,
    preview: Option<(PreviewTarget, Point, Instant)>,
    preview_started: bool,
    /// The pointer was over the control at the last pointer event (hover tint).
    hovered: bool,
}

impl Focusable for Focus {
    fn is_focused(&self) -> bool {
        self.keyboard
    }
    fn focus(&mut self) {
        self.keyboard = true;
    }
    fn unfocus(&mut self) {
        self.keyboard = false;
    }
}

pub struct Control<'a> {
    child: El<'a>,
    target: Option<(&'static str, Cow<'a, str>)>,
    action: Option<Message>,
    hover: bool,
    preload: Option<Message>,
    preview: Option<PreviewTarget>,
    cover: bool,
    scope: bool,
    blocked: bool,
    software_only: Option<&'a Cell<bool>>,
}

impl<'a> Control<'a> {
    pub fn new(child: impl Into<El<'a>>) -> Self {
        Self {
            child: child.into(),
            target: None,
            action: None,
            hover: false,
            preload: None,
            preview: None,
            cover: false,
            scope: false,
            blocked: false,
            software_only: None,
        }
    }

    pub fn target(mut self, kind: &'static str, label: impl Into<Cow<'a, str>>) -> Self {
        self.target = Some((kind, label.into()));
        self
    }

    pub fn action(mut self, action: Message) -> Self {
        self.action = Some(action);
        self
    }

    pub fn tint_on_hover(mut self) -> Self {
        self.hover = true;
        self
    }

    pub fn preload_on_hover(mut self, message: Option<Message>) -> Self {
        self.preload = message;
        self
    }

    pub fn preview_on_hover(mut self, target: PreviewTarget) -> Self {
        self.preview = Some(target);
        self
    }

    pub fn cover(mut self) -> Self {
        self.cover = true;
        self
    }

    /// The child is an iced `scrollable`: report the targets inside it where they are on
    /// screen.
    pub fn scroll_scope(mut self) -> Self {
        self.scope = true;
        self
    }

    pub fn block_keyboard(mut self, blocked: bool) -> Self {
        self.blocked = blocked;
        self
    }

    pub fn software_only(mut self, presenting: &'a Cell<bool>) -> Self {
        self.software_only = Some(presenting);
        self
    }

    pub fn element(self) -> El<'a> {
        El::new(self)
    }
}

impl Widget<Message, Theme, Renderer> for Control<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Focus>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(Focus::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.child)]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.child));
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.child.as_widget().size_hint()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.blocked {
            return;
        }
        if self.action.is_some() {
            operation.focusable(None, layout.bounds(), tree.state.downcast_mut::<Focus>());
        }
        self.child
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.blocked && matches!(event, Event::Keyboard(_)) {
            return;
        }
        let state = tree.state.downcast_mut::<Focus>();
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_))) {
            state.unfocus();
        }
        // iced redraws on pointer motion only when a widget asks; the tint must follow it.
        if self.hover
            && matches!(
                event,
                Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft)
            )
        {
            let over = cursor.is_over(layout.bounds());
            if over != state.hovered {
                state.hovered = over;
                shell.request_redraw();
            }
        }
        if let Some(message) = &self.preload {
            let point = cursor.position_over(layout.bounds());
            match event {
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    state.dwell = point.map(|p| (p, Instant::now() + Duration::from_millis(250)));
                    if let Some((_, at)) = state.dwell {
                        shell.request_redraw_at(at);
                    }
                }
                Event::Window(window::Event::RedrawRequested(now)) => {
                    if let Some((p, at)) = state.dwell {
                        if point != Some(p) {
                            state.dwell = None;
                        } else if *now >= at {
                            state.dwell = None;
                            shell.publish(message.clone());
                        } else {
                            shell.request_redraw_at(at);
                        }
                    }
                }
                Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::CursorLeft) => {
                    state.dwell = None
                }
                _ => {}
            }
        } else {
            state.dwell = None;
        }
        if let Some(target) = self.preview {
            let point = cursor
                .position_over(layout.bounds())
                .filter(|p| viewport.contains(*p));
            let stopped = point.is_none()
                || matches!(
                    event,
                    Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::CursorLeft)
                )
                || state
                    .preview
                    .is_some_and(|(previous, _, _)| previous != target);
            if stopped && let Some((old, _, _)) = state.preview.take() {
                if state.preview_started {
                    shell.publish(Message::PreviewLeave(old));
                }
                state.preview_started = false;
            }
            if let Some(point) = point {
                match event {
                    Event::Mouse(mouse::Event::CursorMoved { .. }) if !state.preview_started => {
                        let deadline = Instant::now() + Duration::from_secs(1);
                        state.preview = Some((target, point, deadline));
                        shell.request_redraw_at(deadline);
                    }
                    Event::Window(window::Event::RedrawRequested(now))
                        if !state.preview_started =>
                    {
                        if let Some((_, expected, deadline)) = state.preview {
                            if point != expected {
                                state.preview = None;
                            } else if *now >= deadline {
                                state.preview_started = true;
                                shell.publish(Message::PreviewEnter(target));
                            } else {
                                shell.request_redraw_at(deadline);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if !shell.is_event_captured()
            && keyboard_mode()
            && state.is_focused()
            && layout.bounds().intersects(viewport)
            && matches!(event, Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(Named::Enter | Named::Space), modifiers, .. }) if modifiers.is_empty())
            && let Some(action) = &self.action
        {
            shell.publish(action.clone());
            shell.capture_event();
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
        if self.software_only.is_some_and(Cell::get) {
            return;
        }
        let bounds = layout.bounds();
        let focused = self.action.is_some()
            && keyboard_mode()
            && tree.state.downcast_ref::<Focus>().is_focused();
        if self.cover {
            crate::probe::cover(bounds);
        }
        if let Some((kind, label)) = &self.target {
            crate::probe::target(kind, label, bounds, *viewport);
        }
        if self.hover && cursor.is_over(bounds) {
            renderer.fill_quad(
                Quad {
                    bounds: bounds.expand(8),
                    border: Border {
                        radius: 12.0.into(),
                        ..Border::default()
                    },
                    ..Quad::default()
                },
                Color::from_rgb8(0x27, 0x27, 0x27),
            );
        }
        let draw = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            )
        };
        if self.scope {
            crate::probe::scoped(bounds, || draw(renderer));
        } else {
            draw(renderer);
        }
        if focused {
            border(renderer, bounds);
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
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

pub fn action<'a>(
    child: impl Into<El<'a>>,
    kind: &'static str,
    label: impl Into<Cow<'a, str>>,
    action: Message,
) -> El<'a> {
    Control::new(child)
        .target(kind, label)
        .action(action)
        .element()
}
