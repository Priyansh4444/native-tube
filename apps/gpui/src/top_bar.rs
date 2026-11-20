//! The top bar: Back, the wordmark, and the search field with its suggestions. Its own cached
//! view: it re-renders only when it is notified (typing, suggestions, focus), never on a scroll or
//! video frame.

use crate::chrome::focus_ring;
use crate::icons::icon;
use crate::probe::{self, Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    Context, Entity, EventEmitter, Focusable as _, HighlightStyle, MouseButton, MouseDownEvent,
    SharedString, StyledText, Subscription, Window, deferred, div, px,
};
use gpui_component::StyledExt;
use gpui_component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use std::time::Duration;

pub enum TopBarEvent {
    Submit(String),
    Back,
    /// ☰: open or close the guide.
    Guide,
}

pub struct TopBar {
    search: Entity<InputState>,
    items: Vec<String>,
    typed: String,
    selected: Option<usize>,
    open: bool,
    seq: u64,
    focused: bool,
    pub can_back: bool,
    /// Window narrower than 656: the field collapses to an icon.
    pub compact: bool,
    expanded: bool,
    slash_at: f64,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TopBarEvent> for TopBar {}

impl TopBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> TopBar {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let subs = vec![cx.subscribe_in(&search, window, Self::on_input)];
        TopBar {
            search,
            items: Vec::new(),
            typed: String::new(),
            selected: None,
            open: false,
            seq: 0,
            focused: false,
            can_back: false,
            compact: false,
            expanded: false,
            slash_at: f64::NEG_INFINITY,
            _subs: subs,
        }
    }

    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.search.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self, cx: &gpui::App) -> Option<SharedString> {
        (!self.compact || self.expanded).then(|| self.search.read(cx).value())
    }

    pub fn suggestions_shown(&self) -> bool {
        self.open && self.focused && !self.items.is_empty() && (!self.compact || self.expanded)
    }

    /// `/` focuses search. The "/" itself can still arrive through the input method after the
    /// field took focus; it is stripped in `on_input` (review P1-11).
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.slash_at = nt_core::metrics::now();
        self.expanded = true;
        self.search.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    pub fn set_query(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.read(cx).value().as_ref() == q {
            return;
        }
        let q = q.to_string();
        self.search.update(cx, |s, cx| s.set_value(q, window, cx));
        self.typed.clear();
        self.items.clear();
        self.open = false;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.selected = None;
        self.seq += 1;
        cx.notify();
    }

    fn submit(&mut self, q: String, window: &mut Window, cx: &mut Context<Self>) {
        let q = q.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.set_query(&q, window, cx);
        self.close(cx);
        self.expanded = false;
        cx.emit(TopBarEvent::Submit(q));
    }

    fn on_input(
        &mut self,
        state: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => {
                let mut text = state.read(cx).value().to_string();
                if nt_core::metrics::now() - self.slash_at < 0.5 && text.starts_with('/') {
                    self.slash_at = f64::NEG_INFINITY;
                    text.remove(0);
                    let t = text.clone();
                    state.update(cx, |s, cx| s.set_value(t, window, cx));
                }
                self.seq += 1;
                self.selected = None;
                let seq = self.seq;
                self.typed = text.clone();
                if text.trim().is_empty() {
                    self.items.clear();
                    self.open = false;
                    cx.notify();
                    return;
                }
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    if this.read_with(cx, |t, _| t.seq != seq).unwrap_or(true) {
                        return;
                    }
                    let result = nt_core::client().suggestions(&text).await;
                    let _ = this.update(cx, |t, cx| {
                        // A newer keystroke, a submit, or a blur makes this answer stale.
                        if t.seq != seq || !t.focused {
                            return;
                        }
                        match result {
                            Ok(mut items) => {
                                items.truncate(10);
                                t.open = !items.is_empty();
                                t.items = items;
                                t.selected = None;
                                cx.notify();
                            }
                            Err(e) => eprintln!("nt-gpui: suggestions failed: {e}"),
                        }
                    });
                })
                .detach();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let q = match self.selected.and_then(|i| self.items.get(i)) {
                    Some(s) if self.open => s.clone(),
                    _ => state.read(cx).value().to_string(),
                };
                self.submit(q, window, cx);
            }
            InputEvent::Focus => {
                self.focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                // Losing the window's activation blurs too; only a real blur closes the list.
                if window.is_window_active() {
                    self.focused = false;
                    self.close(cx);
                    if self.compact && self.search.read(cx).value().is_empty() {
                        self.expanded = false;
                    }
                }
            }
        }
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if !self.open || self.items.is_empty() {
            return false;
        }
        let n = self.items.len();
        self.selected = Some(match (self.selected, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        });
        cx.notify();
        true
    }

    fn suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !(self.open && self.focused) || self.items.is_empty() {
            return None;
        }
        let typed = self.typed.trim_start().to_lowercase();
        let mut list = div()
            .id("suggestions")
            .children(target(cx, Layer::Suggestions, "_panel", ""))
            .occlude()
            .absolute()
            .top(px(44.0))
            .left_0()
            .right(px(64.0))
            .py(px(16.0))
            .rounded(px(12.0))
            .bg(surface())
            .shadow_lg()
            .flex()
            .flex_col();
        for (i, s) in self.items.iter().enumerate() {
            let q = s.clone();
            let selected = self.selected == Some(i);
            // The typed prefix at 400, the completion at 500, as YouTube draws it.
            let split = if s.to_lowercase().starts_with(&typed) {
                typed.len().min(s.len())
            } else {
                0
            };
            let split = (0..=split)
                .rev()
                .find(|i| s.is_char_boundary(*i))
                .unwrap_or(0);
            let styled = StyledText::new(SharedString::from(s.clone())).with_highlights(
                (split < s.len()).then(|| {
                    (
                        split..s.len(),
                        HighlightStyle {
                            font_weight: Some(W500),
                            ..Default::default()
                        },
                    )
                }),
            );
            list = list.child(
                div()
                    .id(("sugg", i))
                    .children(target(cx, Layer::Suggestions, "suggestion", s.as_str()))
                    .h(px(32.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(fill_hover()))
                    .hover(|s| s.bg(fill_hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.submit(q.clone(), window, cx);
                        }),
                    )
                    .child(icon("search", 20.0, text()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(16.0))
                            .line_height(px(22.0))
                            .font_weight(W400)
                            .text_color(text())
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(styled),
                    ),
            );
        }
        Some(deferred(list).with_priority(1))
    }

    fn search_box(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_text = !self.search.read(cx).value().is_empty();
        let border = if self.focused {
            search_focus()
        } else {
            search_border()
        };
        let keyboard = self.is_focused(window, cx) && window.last_input_was_keyboard();
        let field = div()
            .children(target(cx, Layer::Chrome, "search_field", "Search"))
            .when(keyboard, |d| {
                d.refine_style(&focus_ring(Default::default()))
            })
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .pl(px(if self.focused { 16.0 } else { 20.0 }))
            .pr(px(4.0))
            .bg(search_bg())
            .border_1()
            .border_color(border)
            .rounded_l(px(20.0))
            .when(self.focused, |d| d.child(icon("search", 20.0, text())))
            .child(
                Input::new(&self.search)
                    .appearance(false)
                    .tab_index(2)
                    .w_full()
                    .text_size(px(16.0)),
            )
            .when(has_text, |d| {
                d.child(
                    div()
                        .id("search-clear")
                        .tab_index(3)
                        .focus_visible(focus_ring)
                        .children(target(cx, Layer::Chrome, "clear", "Clear search"))
                        .size(px(40.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(fill()))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.search.update(cx, |s, cx| {
                                s.set_value("", window, cx);
                                s.focus(window, cx);
                            });
                            this.close(cx);
                        }))
                        .child(icon("close", 24.0, text())),
                )
            });
        let go = div()
            .id("search-go")
            .tab_index(4)
            .focus_visible(focus_ring)
            .children(target(cx, Layer::Chrome, "search_button", "Search"))
            .w(px(64.0))
            .h_full()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .bg(search_btn())
            .border_1()
            .border_l_0()
            .border_color(search_border())
            .rounded_r(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(c(0x303030)))
            .on_click(cx.listener(|this, _, window, cx| {
                let q = this.search.read(cx).value().to_string();
                this.submit(q, window, cx);
            }))
            .child(icon("search", 24.0, text()));
        div()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.move_selection(false, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.move_selection(true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.open {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .relative()
            .w_full()
            .max_w(px(640.0))
            .h(px(40.0))
            .flex()
            .child(field)
            .child(go)
            .children(self.suggestions(cx))
    }

    fn round_button(
        &self,
        id: &'static str,
        name: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .size(px(40.0))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_full()
            .when(enabled, |d| {
                let (kind, label) = match id {
                    "guide" => ("guide", "Guide"),
                    "back" => ("back", "Back"),
                    "search-open" => ("search_button", "Search"),
                    _ => ("back", "Close search"),
                };
                d.tab_index(if id == "guide" { 0 } else { 1 })
                    .focus_visible(focus_ring)
                    .children(target(cx, Layer::Chrome, kind, label))
                    .cursor_pointer()
                    .hover(|s| s.bg(fill()))
                    .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
            })
            .child(icon(name, 24.0, if enabled { text() } else { c(0x5f5f5f) }))
    }
}

impl Render for TopBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        probe::begin(cx, &[Layer::Chrome, Layer::Suggestions]);
        let bar = div()
            .id("top-bar")
            .tab_group()
            .tab_index(1)
            .tab_stop(false)
            .size_full()
            .bg(bg())
            .flex()
            .items_center()
            .px(px(16.0));
        if self.compact && self.expanded {
            // Below 656 the field expands over the whole bar.
            return bar
                .gap(px(8.0))
                .child(self.round_button(
                    "search-collapse",
                    "arrow_back",
                    true,
                    cx,
                    |this, _, cx| {
                        this.expanded = false;
                        this.close(cx);
                    },
                ))
                .child(div().flex_1().child(self.search_box(window, cx)));
        }
        let left = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .child(self.round_button("guide", "menu", true, cx, |_, _, cx| {
                cx.emit(TopBarEvent::Guide)
            }))
            .child(
                self.round_button("back", "arrow_back", self.can_back, cx, |_, _, cx| {
                    cx.emit(TopBarEvent::Back)
                }),
            )
            .child(
                txt("NativeTube", 20.0, 28.0, W700, text())
                    .ml(px(8.0))
                    .whitespace_nowrap(),
            );
        if self.compact {
            return bar.justify_between().child(left).child(self.round_button(
                "search-open",
                "search",
                true,
                cx,
                |this, window, cx| {
                    this.expanded = true;
                    this.search.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                },
            ));
        }
        bar.child(left.w(px(224.0)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(200.0))
                    .px(px(16.0))
                    .flex()
                    .justify_center()
                    .child(self.search_box(window, cx)),
            )
            .child(div().w(px(224.0)).flex_shrink(1.0))
    }
}
