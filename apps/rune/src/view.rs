//! The view: top bar, navigation rail, chip bar, the page's scroller (only mounted blocks are
//! built), the search dropdown, menus and toasts, and every feed block. The watch page's blocks
//! are in `watch.rs`.

use crate::app::{App, CardMenu};
use crate::pages::*;
use crate::scroller::BAR_W;
use crate::theme::*;
use crate::watch;
use nt_core::{Video, library};
use rune::{
    Color, Cursor, Cx, El, Icon, Phase, PointerButton, Span, SurfaceFit, Tx, col, el, icon, rich,
    row, text,
};
use rune_kit::{Spinner, TextInput};
use std::sync::OnceLock;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let scale = cx.env().scale;
    s.scale.set(scale);
    let tok = t();
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(if s.fullscreen { Color::BLACK } else { tok.bg })
        .on_layout(|s: &mut App, r: [f32; 4]| {
            s.win_w = r[2];
            s.win_h = r[3];
        })
        .on_wheel(|s: &mut App, e, tx| s.on_wheel(e, tx))
        .pointer_buttons([PointerButton::Back, PointerButton::Forward])
        .on_press_tx(|s: &mut App, tx: &mut Tx| match tx.button() {
            PointerButton::Back => s.back(tx),
            PointerButton::Forward => s.go_forward(tx),
            _ => {}
        });
    if s.fullscreen && s.top().watch().is_some() {
        root = root.child(watch::player(s, s.win_w, s.win_h, true));
    } else {
        let page = s.top();
        let mut main = col().key("main").grow(1.0).h_full().min_w(0.0);
        if page.has_chips() {
            main = main.child(chip_bar(s));
        }
        main = main.child(viewport(s, scale));
        let mut body = row().key("body").w_full().grow(1.0).min_h(0.0);
        if page.has_rail(s.win_w) {
            body = body.child(rail(s));
        }
        root = root.child(top_bar(s)).child(body.child(main));
        if s.sugg_open
            && let Some(r) = cx.layout("searchbox-frame")
        {
            root = root.child(dropdown(s, r));
        }
    }
    if s.video.menu != crate::video::Menu::Closed
        && s.video.controls
        && let Some(r) = player_rect(s)
    {
        root = root.child(watch::settings_layer(s, r));
    }
    if let Some(m) = &s.card_menu {
        root = root.child(card_menu(s, m));
    }
    if let Some((msg, _)) = &s.toast {
        root = root.child(toast(msg, s.win_h));
    }
    s.images.end_view();
    root
}

// ----- shared pieces ------------------------------------------------------------

/// Keyboard focus: a 2 px #f1f1f1 ring at offset 2, shown for keyboard focus only.
pub fn focus(e: El) -> El {
    e.focusable(true).focus_ring(2.0, 2.0, t().text)
}

/// A round icon button (40 px target, 24 px icon) with a hover fill.
pub fn icon_button(key: &str, ic: &'static Icon, label: &'static str, color: Color) -> El {
    focus(
        el().key(key.to_string())
            .size(40.0, 40.0)
            .radius(20.0)
            .center()
            .role("button")
            .label(label)
            .cursor(Cursor::Pointer)
            .hover(t().fill)
            .press(t().fill_hover)
            .child(icon(ic, 24.0).color(color)),
    )
}

pub fn px(w: f32, scale: f32) -> u32 {
    (w * scale).ceil().max(1.0) as u32
}

pub fn avatar(s: &App, url: Option<&str>, d: f32, scale: f32) -> El {
    let a = el().size(d, d).radius(d / 2.0).fill(t().skeleton);
    match url {
        Some(u) if !u.is_empty() => a.pixels_fit(&s.images.get(u, px(d, scale)), SurfaceFit::Fill),
        _ => a,
    }
}

/// A pill button (36 tall, 16 px padding): the primary (#f1f1f1) or a fill one.
pub fn pill(label: &str, ic: Option<&'static Icon>, primary: bool) -> El {
    let tok = t();
    let (bg, fg) = if primary {
        (tok.chip_sel_bg, tok.chip_sel_fg)
    } else {
        (tok.fill, tok.text)
    };
    let mut b = row()
        .h(36.0)
        .px(16.0)
        .gap(6.0)
        .radius(18.0)
        .center_y()
        .fill(bg)
        .role("button")
        .label(label.to_string())
        .cursor(Cursor::Pointer)
        .hover(if primary {
            Color::BLACK.alpha(0.1)
        } else {
            tok.fill
        })
        .press(Color::BLACK.alpha(0.15));
    if let Some(i) = ic {
        b = b.pl(12.0).child(icon(i, 24.0).color(fg));
    }
    focus(b.child(text(label.to_string()).font(S14M).color(fg)))
}

pub fn subscribe_button(ch: nt_core::ChannelRef) -> El {
    let subscribed = library().is_subscribed(&ch.id);
    let b = if subscribed {
        pill("Subscribed", Some(&BELL), false)
    } else {
        pill("Subscribe", None, true)
    };
    b.key("subscribe").on_press(move |s: &mut App| {
        library().toggle_subscription(&ch);
        s.top().touch();
    })
}

fn retry_button() -> El {
    pill("Retry", None, false)
        .key("retry")
        .on_press(|s: &mut App| s.retry())
}

/// The duration badge (or LIVE), inset 8 from the thumbnail's bottom-right.
fn badge(v: &Video) -> Option<El> {
    let tok = t();
    let (label, bg) = if v.is_live {
        ("LIVE".to_string(), tok.red)
    } else {
        (v.duration.clone()?, tok.badge)
    };
    Some(
        el().px(4.0)
            .py(1.0)
            .radius(4.0)
            .fill(bg)
            .child(text(label).font(S12M).color(tok.text)),
    )
}

/// A thumbnail: placeholder fill, the image cover-cropped, the badge, the watched bar, and a
/// 10 % darkening while pressed.
pub fn thumb(s: &App, v: &Video, w: f32, h: f32, radius: f32, scale: f32) -> El {
    let tok = t();
    let mut th = el()
        .w(w)
        .h(h)
        .radius(radius)
        .fill(tok.skeleton)
        .pixels_fit(&s.images.get(&v.thumbnail, px(w, scale)), SurfaceFit::Fill)
        .clip_rounded(radius);
    if let Some(b) = badge(v) {
        th = th.child(
            col()
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .padding(8.0)
                .justify_end()
                .items_end()
                .child(b),
        );
    }
    if let Some(f) = s.progress.get(&v.id) {
        th = th.child(
            el().absolute(0.0, h - 4.0)
                .w(w)
                .h(4.0)
                .fill(Color::WHITE.alpha(0.3))
                .child(el().w((w * f).max(4.0)).h(4.0).fill(tok.red)),
        );
    }
    th
}

/// The watch page's player box in window points, while a watch page shows.
fn player_rect(s: &App) -> Option<[f32; 4]> {
    let page = s.top();
    page.watch()?;
    if s.fullscreen {
        return Some([0.0, 0.0, s.win_w, s.win_h]);
    }
    let p = page.plan(s.geo()).player;
    let [x, y] = content_to_window(s, p[0], p[1]);
    Some([x, y, p[2], p[3]])
}

/// The window position of a point in the scroller's content.
fn content_to_window(s: &App, x: f32, y: f32) -> [f32; 2] {
    let page = s.top();
    let left = if page.has_rail(s.win_w) { RAIL } else { 0.0 };
    let top = TOP_BAR + if page.has_chips() { CHIP_BAR } else { 0.0 };
    [left + x, top + y - page.scroll.anim.offset() as f32]
}

/// The "⋮" button: shown while its card or row is hovered (or focused), opens the card menu.
pub fn more_button(v: &Video, at: [f32; 2]) -> El {
    let v = v.clone();
    icon_button("more", &MORE, "More actions", t().text)
        .group_opacity(0.0, 1.0)
        .on_press(move |s: &mut App| {
            let p = content_to_window(s, at[0], at[1]);
            s.card_menu = Some(CardMenu {
                video: v.clone(),
                at: p,
                at_pointer: false,
            });
        })
}

/// Right-click on a video: its menu at the pointer (YouTube's "⋮" menu, not a browser's).
pub fn context_menu(v: &Video) -> impl Fn(&mut App, rune::PointerPress, &mut Tx) + 'static {
    let v = v.clone();
    move |s: &mut App, p: rune::PointerPress, tx: &mut Tx| {
        s.cancel_fling(tx);
        s.card_menu = Some(CardMenu {
            video: v.clone(),
            at: [p.x, p.y],
            at_pointer: true,
        });
    }
}

// ----- chrome -----------------------------------------------------------------

fn top_bar(s: &App) -> El {
    let tok = t();
    let narrow = s.win_w < 656.0;
    let mut bar = row()
        .key("topbar")
        .w_full()
        .h(TOP_BAR)
        .px(16.0)
        .gap(8.0)
        .center_y()
        .fill(tok.bg);
    if narrow && s.search_open {
        // Collapsed search, expanded over the bar.
        return bar
            .child(
                icon_button("search-close", &BACK, "Close search", tok.text).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.search_open = false;
                        s.close_suggestions();
                        tx.blur();
                    },
                ),
            )
            .child(search_box(s));
    }
    let back = if s.can_back() {
        icon_button("back", &BACK, "Back", tok.text)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.back(tx))
    } else {
        el().key("back")
            .size(40.0, 40.0)
            .center()
            .child(icon(&BACK, 24.0).color(tok.text.alpha(0.3)))
    };
    // Equal sides, so the search box centres in the window as YouTube's does.
    let side = if s.win_w < 900.0 { 170.0 } else { 200.0 };
    let left = row()
        .key("brand")
        .w(side)
        .gap(8.0)
        .center_y()
        .shrink()
        .child(back)
        .child(
            text("NativeTube")
                .font(S20B)
                .color(tok.text)
                .cursor(Cursor::Pointer)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.root(Kind::Home { topic: 0 }, tx)),
        );
    bar = bar.child(left);
    if narrow {
        bar.child(el().grow(1.0)).child(
            icon_button("search-open", &SEARCH, "Search", tok.text).on_press_tx(
                |s: &mut App, tx: &mut Tx| {
                    s.search_open = true;
                    tx.focus("input", false);
                },
            ),
        )
    } else {
        bar.child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .justify_center()
                .px(16.0)
                .child(search_box(s)),
        )
        .child(el().w(side).shrink())
    }
}

fn search_box(s: &App) -> El {
    let tok = t();
    let focused = s.search.focused;
    let field = TextInput::new(&s.search)
        .placeholder("Search")
        .label("Search")
        .plain()
        .width(None)
        .height(38.0)
        .text_style(S16)
        .on_edit(|s: &mut App| &mut s.search)
        .on_change(|s: &mut App, tx: &mut Tx| s.search_changed(tx))
        .on_key(|s: &mut App, ev, tx| s.search_key(ev, tx))
        .on_submit(|s: &mut App, tx: &mut Tx| s.submit_search(tx))
        .build(input_kit());
    let mut frame = row()
        .key("searchbox-frame")
        .grow(1.0)
        .min_w(0.0)
        .h(40.0)
        .pl(if focused { 16.0 } else { 4.0 })
        .gap(4.0)
        .center_y()
        .fill(tok.field_bg)
        .border(
            1.0,
            if focused {
                tok.field_focus
            } else {
                tok.field_border
            },
        )
        .corners([20.0, 0.0, 0.0, 20.0])
        .cursor(Cursor::Text);
    if focused {
        frame = frame.child(icon(&SEARCH, 20.0).color(tok.text));
    }
    frame = frame.child(el().key("searchbox").grow(1.0).min_w(0.0).child(field));
    let button = focus(
        el().key("search-go")
            .size(64.0, 40.0)
            .fill(tok.button_bg)
            .border(1.0, tok.field_border)
            .corners([0.0, 20.0, 20.0, 0.0])
            .center()
            .role("button")
            .label("Search")
            .cursor(Cursor::Pointer)
            .hover(tok.fill)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.submit_search(tx))
            .child(icon(&SEARCH, 24.0).color(tok.text)),
    );
    row()
        .key("search")
        .grow(1.0)
        .max_w(640.0)
        .min_w(0.0)
        .center_y()
        .child(frame)
        .child(button)
}

fn rail(s: &App) -> El {
    let tok = t();
    let shown = if s.can_back() {
        None
    } else {
        match s.top().kind {
            Kind::Home { .. } => Some(0),
            Kind::Subs { .. } => Some(1),
            Kind::History => Some(2),
            Kind::Later => Some(3),
            _ => None,
        }
    };
    let item = |i: usize,
                ic: &'static Icon,
                fill: &'static Icon,
                label: &'static str,
                kind: fn() -> Kind| {
        let on = shown == Some(i);
        focus(
            col()
                .key(format!("rail-{i}"))
                .w(64.0)
                .h(74.0)
                .pt(16.0)
                .gap(6.0)
                .items_center()
                .radius(10.0)
                .role("link")
                .label(label)
                .cursor(Cursor::Pointer)
                .hover(tok.fill)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.root(kind(), tx))
                .child(icon(if on { fill } else { ic }, 24.0).color(tok.text))
                .child(text(label).font(S10).color(tok.text)),
        )
    };
    col()
        .key("rail")
        .w(RAIL)
        .h_full()
        .pt(4.0)
        .items_center()
        .child(item(0, &HOME, &HOME_FILL, "Home", || Kind::Home {
            topic: 0,
        }))
        .child(item(1, &SUBS, &SUBS_FILL, "Subscriptions", || Kind::Subs {
            channels: Vec::new(),
        }))
        .child(item(2, &HISTORY, &HISTORY_FILL, "History", || {
            Kind::History
        }))
        .child(item(3, &LATER, &LATER_FILL, "Watch Later", || Kind::Later))
}

/// The chips' widths: text + 24 padding each, 12 apart.
pub fn chips_width() -> f32 {
    static W: OnceLock<f32> = OnceLock::new();
    *W.get_or_init(|| {
        let n = nt_core::TOPICS.len() as f32;
        nt_core::TOPICS
            .iter()
            .map(|t| rune::text::measure(t.title, &[], S14M, None).0 + 24.0)
            .sum::<f32>()
            + 12.0 * (n - 1.0).max(0.0)
    })
}

fn chip_bar(s: &App) -> El {
    let tok = t();
    let cur = match s.top().kind {
        Kind::Home { topic } => topic,
        _ => 0,
    };
    let pad = s.geo().pad();
    let room = (chips_width() - (s.vw - 2.0 * pad)).max(0.0);
    let mut chips = row()
        .key("chips")
        .gap(12.0)
        .center_y()
        .live_offset(&s.chips_live);
    for (i, topic) in nt_core::TOPICS.iter().enumerate() {
        let on = i == cur;
        chips = chips.child(focus(
            el().key(format!("chip-{i}"))
                .h(32.0)
                .px(12.0)
                .radius(8.0)
                .center()
                .role("button")
                .label(topic.title)
                .cursor(Cursor::Pointer)
                .fill(if on { tok.chip_sel_bg } else { tok.fill })
                .hover(if on { Color::TRANSPARENT } else { tok.fill })
                .on_press(move |s: &mut App| s.set_topic(i))
                .child(text(topic.title).font(S14M).color(if on {
                    tok.chip_sel_fg
                } else {
                    tok.text
                })),
        ));
    }
    let mut bar = el()
        .key("chipbar")
        .w_full()
        .h(CHIP_BAR)
        .px(pad)
        .center_y()
        .fill(tok.bg)
        .clip_soft(0.0)
        .child(chips);
    let arrow = |left: bool| {
        let fade = if left {
            [(0.0, tok.bg), (0.6, tok.bg), (1.0, tok.bg.alpha(0.0))]
        } else {
            [(0.0, tok.bg.alpha(0.0)), (0.4, tok.bg), (1.0, tok.bg)]
        };
        row()
            .h(CHIP_BAR)
            .w(90.0)
            .center_y()
            .linear_gradient(std::f32::consts::FRAC_PI_2, fade)
            .child(if left { el().w(0.0) } else { el().grow(1.0) })
            .child(
                icon_button(
                    if left { "chips-left" } else { "chips-right" },
                    if left { &CHEVRON_LEFT } else { &CHEVRON_RIGHT },
                    if left { "Previous" } else { "Next" },
                    tok.text,
                )
                .on_press(move |s: &mut App| s.scroll_chips(if left { -300.0 } else { 300.0 })),
            )
    };
    if s.chips_x > 0.5 {
        bar = bar.child(arrow(true).absolute(0.0, 0.0));
    }
    if s.chips_x < room - 0.5 {
        bar = bar.child(arrow(false).absolute((s.vw - 90.0).max(0.0), 0.0));
    }
    bar
}

fn dropdown(s: &App, r: [f32; 4]) -> El {
    let tok = t();
    let mut list = col()
        .key("suggest")
        .absolute(r[0], r[1] + r[3] + 4.0)
        .w(r[2])
        .py(16.0)
        .radius(12.0)
        .fill(tok.surface)
        .shadow(4.0, 32.0, Color::BLACK.alpha(0.5));
    let typed = s.sugg_for.to_lowercase();
    for (i, q) in s.sugg.iter().enumerate() {
        let sel = s.sugg_sel == Some(i);
        // The typed prefix at 400, the completion at 500.
        let split = if q.to_lowercase().starts_with(&typed) {
            typed.len().min(q.len())
        } else {
            0
        };
        let split = if q.is_char_boundary(split) { split } else { 0 };
        let label = rich(q.clone(), vec![Span::new(split, q.len()).weight(500.0)])
            .font(S16)
            .color(tok.text)
            .truncate();
        list = list.child(
            row()
                .key(format!("sg-{i}"))
                .w_full()
                .h(32.0)
                .px(16.0)
                .gap(16.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .fill(if sel { tok.fill } else { Color::TRANSPARENT })
                .hover(tok.fill)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.choose_suggestion(i, tx))
                .child(icon(&SEARCH, 20.0).color(tok.text))
                .child(label),
        );
    }
    list
}

fn card_menu(s: &App, m: &CardMenu) -> El {
    let tok = t();
    let saved = library().is_saved(&m.video.id);
    let item = |key: &str, ic: &'static Icon, label: &'static str| {
        row()
            .key(key.to_string())
            .w_full()
            .h(40.0)
            .px(16.0)
            .gap(16.0)
            .center_y()
            .role("menuitem")
            .label(label)
            .cursor(Cursor::Pointer)
            .hover(tok.fill)
            .focusable(true)
            .focus_highlight(tok.fill)
            .child(icon(ic, 24.0).color(tok.text))
            .child(text(label).font(S14).color(tok.text))
    };
    let (v0, v1, v2, v3) = (
        m.video.clone(),
        m.video.clone(),
        m.video.clone(),
        m.video.clone(),
    );
    let mut menu = col()
        .key("card-menu")
        .role("menu")
        .w(MENU_W)
        .py(8.0)
        .radius(12.0)
        .fill(tok.menu)
        .shadow(4.0, 32.0, Color::BLACK.alpha(0.5))
        .focus_group()
        // A press on the menu's padding does nothing (and nothing underneath).
        .on_press(|_: &mut App| {})
        .child(item("menu-play", &PLAY_OUTLINE, "Play").on_press_tx(
            move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.open_video(v0.clone(), tx);
            },
        ))
        .child(
            item(
                "menu-later",
                if saved { &DELETE } else { &LATER },
                if saved {
                    "Remove from Watch Later"
                } else {
                    "Save to Watch Later"
                },
            )
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.toggle_later(&v1, tx);
            }),
        )
        .child(item("menu-copy", &SHARE, "Copy link").on_press_tx(
            move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.share(&v2.id, tx);
            },
        ));
    let with_channel = v3.channel_id.as_deref().is_some_and(|c| !c.is_empty());
    if let Some(cid) = v3.channel_id.clone().filter(|_| with_channel) {
        menu = menu.child(item("menu-channel", &SUBS, "Go to channel").on_press_tx(
            move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.open_channel(cid.clone(), tx);
            },
        ));
    }
    let h = 16.0 + 40.0 * if with_channel { 4.0 } else { 3.0 };
    let [x, y] = menu_origin(m, h, s.win_w, s.win_h);
    // A full-window catcher: a press (either button) anywhere else only closes the menu.
    el().key("menu-layer")
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .on_press(|s: &mut App| s.card_menu = None)
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.card_menu = None)
        .child(menu.absolute(x, y))
}

pub const MENU_W: f32 = 248.0;

/// Where a menu `h` tall goes so it stays in a `win_w` × `win_h` window: at the pointer
/// (flipped left or up near the right or bottom edge), or under the "⋮" button, right-aligned.
pub fn menu_origin(m: &CardMenu, h: f32, win_w: f32, win_h: f32) -> [f32; 2] {
    let max_x = (win_w - MENU_W - 8.0).max(8.0);
    let max_y = (win_h - h - 8.0).max(8.0);
    let (x, y) = if m.at_pointer {
        let x = if m.at[0] + MENU_W > win_w - 8.0 {
            m.at[0] - MENU_W
        } else {
            m.at[0]
        };
        let y = if m.at[1] + h > win_h - 8.0 {
            m.at[1] - h
        } else {
            m.at[1]
        };
        (x, y)
    } else {
        let x = m.at[0] + 40.0 - MENU_W;
        let y = if m.at[1] + 40.0 + h > win_h - 8.0 {
            m.at[1] - h
        } else {
            m.at[1] + 40.0
        };
        (x, y)
    };
    [x.clamp(8.0, max_x), y.clamp(8.0, max_y)]
}

fn toast(msg: &str, win_h: f32) -> El {
    let tok = t();
    el().key("toast")
        .absolute(24.0, (win_h - 72.0).max(0.0))
        .px(16.0)
        .py(12.0)
        .radius(8.0)
        .fill(tok.text)
        .child(text(msg.to_string()).font(S14).color(tok.bg))
}

// ----- the scroller -----------------------------------------------------------

fn viewport(s: &App, scale: f32) -> El {
    let page = s.top();
    let g = s.geo();
    let plan = page.plan(g);
    let (top, bottom) = page.scroll.mounted();
    let mut content = el()
        .key(format!("content-{}", page.id))
        .w_full()
        .h(plan.height.max(g.vh))
        .live_offset(&page.scroll.live);
    for b in plan.visible(top, bottom) {
        content = content.child(
            block(s, page, &plan, b, scale)
                .absolute(b.x, b.y)
                .w(b.w)
                .h(b.h),
        );
    }
    let tok = t();
    let bar = el()
        .key("scrollbar")
        .absolute((g.vw - BAR_W - 4.0).max(0.0), 0.0)
        .w(BAR_W + 4.0)
        .h(g.vh)
        .live_opacity(&page.scroll.bar_alpha)
        .on_hover_change(|s: &mut App, on: bool, tx: &mut Tx| {
            let now = crate::scroller::now();
            s.top_mut().scroll.hover(on, now);
            tx.live_only();
            tx.wake_after(std::time::Duration::ZERO);
        })
        .on_drag(|s: &mut App, d: &mut rune::Drag, tx: &mut Tx| {
            let sc = &mut s.top_mut().scroll;
            if d.phase == Phase::Down {
                let th = sc.thumb_rect();
                sc.grab = if d.y >= th[1] && d.y <= th[1] + th[3] {
                    d.y - th[1]
                } else {
                    th[3] / 2.0
                };
            }
            sc.drag_to(d.y);
            tx.live_only();
            tx.wake_after(std::time::Duration::ZERO);
        })
        .child(
            el().key("thumb")
                .absolute(2.0, 0.0)
                .w(BAR_W)
                .h(40.0)
                .radius(BAR_W / 2.0)
                .fill(tok.text2.alpha(0.6))
                .live_rect(&page.scroll.thumb),
        );
    el().key("viewport")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .clip_soft(0.0)
        .on_layout(|s: &mut App, r: [f32; 4]| {
            s.vw = r[2];
            s.vh = r[3];
        })
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.cancel_fling(tx))
        .child(content)
        .child(bar)
}

fn block(s: &App, page: &Page, plan: &Plan, b: &Block, scale: f32) -> El {
    let items = &page.feed.items;
    match b.kind {
        B::Card(i) => items
            .get(i)
            .map_or_else(el, |v| card(s, v, i, b, plan.card_channel, scale)),
        B::SkelCard(i) => skel_card(i, b.w, plan.card_channel),
        B::Row(i) => items.get(i).map_or_else(el, |v| {
            let query = match &page.kind {
                Kind::Search { query } => Some(query.as_str()),
                _ => None,
            };
            list_row(s, v, i, b, plan.row_thumb, query, scale)
        }),
        B::SkelRow(i) => skel_row(i, plan.row_thumb),
        B::Compact(i) => items
            .get(i)
            .map_or_else(el, |v| watch::compact(s, v, i, b, scale)),
        B::SkelCompact(i) => watch::skel_compact(i),
        B::Status(st) => status(page, st),
        B::Banner => banner(s, page, b.w, scale),
        B::ChannelHead => channel_head(s, page, scale),
        B::Tabs => tabs(),
        B::Strip => strip(s, page, scale),
        B::LibHead => lib_head(page),
        B::Player => watch::player(s, b.w, b.h, false),
        B::Info => watch::info(s, page, b, scale),
        B::SkelInfo => watch::skel_info(b.w),
        B::Desc => watch::description(page, plan, b),
        B::CommentsHead => watch::comments_head(page),
        B::Comment(i) => page
            .watch()
            .and_then(|w| w.comments.get(i))
            .map_or_else(el, |c| watch::comment(s, page, c, i, b, scale)),
    }
}

// ----- feed blocks --------------------------------------------------------------

fn open_handler(v: &Video) -> impl Fn(&mut App, &mut Tx) + 'static {
    let v = v.clone();
    move |s: &mut App, tx: &mut Tx| {
        if s.press_ok(tx) {
            s.open_video(v.clone(), tx);
        }
    }
}

fn channel_handler(v: &Video) -> impl Fn(&mut App, &mut Tx) + 'static {
    let id = v.channel_id.clone().unwrap_or_default();
    move |s: &mut App, tx: &mut Tx| {
        if s.press_ok(tx) {
            s.open_channel(id.clone(), tx);
        }
    }
}

fn card(s: &App, v: &Video, i: usize, b: &Block, with_channel: bool, scale: f32) -> El {
    let tok = t();
    let w = b.w;
    let th = w * 9.0 / 16.0;
    let mut text_col = col()
        .grow(1.0)
        .min_w(0.0)
        .cursor(Cursor::Pointer)
        .on_press_tx(open_handler(v))
        .child(
            text(v.title.clone())
                .font(S16M)
                .color(tok.text)
                .max_lines(2)
                .mb(4.0),
        );
    let mut info = row().w_full().pt(12.0).gap(12.0);
    if with_channel {
        if v.channel_avatar.is_some() {
            info = info.child(
                avatar(s, v.channel_avatar.as_deref(), 36.0, scale)
                    .cursor(Cursor::Pointer)
                    .on_press_tx(channel_handler(v)),
            );
        }
        text_col = text_col.child(
            text(v.channel.clone())
                .font(S14)
                .color(tok.text2)
                .truncate()
                .on_press_tx(channel_handler(v)),
        );
    }
    text_col = text_col.child(text(v.meta_line()).font(S14).color(tok.text2).truncate());
    info = info
        .child(text_col)
        .child(more_button(v, [b.x + w - 40.0, b.y + th + 12.0]).mr(-12.0));
    col()
        .key(format!("card-{i}-{}", v.id))
        .w(w)
        .hover_group()
        .on_secondary_press(context_menu(v))
        .child(focus(
            thumb(s, v, w, th, 12.0, scale)
                .role("link")
                .label(v.title.clone())
                .cursor(Cursor::Pointer)
                .press(Color::BLACK.alpha(0.1))
                .on_press_tx(open_handler(v)),
        ))
        .child(info)
}

fn skel_card(i: usize, w: f32, with_channel: bool) -> El {
    let tok = t();
    let bar = |w: f32| el().w(w).h(16.0).radius(4.0).fill(tok.skeleton);
    let mut info = row().w_full().pt(12.0).gap(12.0);
    if with_channel {
        info = info.child(el().size(36.0, 36.0).radius(18.0).fill(tok.skeleton));
    }
    col()
        .key(format!("skel-{i}"))
        .w(w)
        .child(el().w(w).h(w * 9.0 / 16.0).radius(12.0).fill(tok.skeleton))
        .child(
            info.child(
                col()
                    .grow(1.0)
                    .gap(8.0)
                    .pt(2.0)
                    .child(bar(w * 0.75))
                    .child(bar(w * 0.45)),
            ),
        )
}

/// Search-style row; `query` makes its terms stand out in the snippet.
fn list_row(
    s: &App,
    v: &Video,
    i: usize,
    b: &Block,
    (tw, th): (f32, f32),
    query: Option<&str>,
    scale: f32,
) -> El {
    let tok = t();
    let mut chan = row().gap(8.0).py(12.0).center_y().cursor(Cursor::Pointer);
    if v.channel_avatar.is_some() {
        chan = chan.child(avatar(s, v.channel_avatar.as_deref(), 24.0, scale));
    }
    chan = chan.child(
        text(v.channel.clone())
            .font(S12)
            .color(tok.text2)
            .truncate(),
    );
    if v.channel_id.is_some() {
        chan = chan.on_press_tx(channel_handler(v));
    }
    let mut info = col()
        .grow(1.0)
        .min_w(0.0)
        .cursor(Cursor::Pointer)
        .on_press_tx(open_handler(v))
        .child(
            row()
                .w_full()
                .child(
                    text(v.title.clone())
                        .font(S18)
                        .color(tok.text)
                        .max_lines(2)
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(more_button(v, [b.x + b.w - 40.0, b.y])),
        )
        .child(text(v.meta_line()).font(S12).color(tok.text2).truncate())
        .child(chan);
    if let Some(sn) = &v.snippet {
        info = info.child(snippet(sn, query));
    }
    row()
        .key(format!("row-{i}-{}", v.id))
        .w_full()
        .h(th)
        .gap(16.0)
        .hover_group()
        .on_secondary_press(context_menu(v))
        .child(focus(
            thumb(s, v, tw, th, 12.0, scale)
                .role("link")
                .label(v.title.clone())
                .cursor(Cursor::Pointer)
                .press(Color::BLACK.alpha(0.1))
                .on_press_tx(open_handler(v)),
        ))
        .child(info)
}

/// One line of snippet with the query's words at 500 in the primary colour.
fn snippet(sn: &str, query: Option<&str>) -> El {
    let tok = t();
    let lower = sn.to_lowercase();
    let mut spans = Vec::new();
    // Only when lowercasing keeps byte offsets (ASCII and most Latin text).
    if let Some(q) = query.filter(|_| lower.len() == sn.len()) {
        for word in q.split_whitespace().filter(|w| w.len() > 1) {
            let word = word.to_lowercase();
            let mut from = 0;
            while let Some(pos) = lower.get(from..).and_then(|rest| rest.find(&word)) {
                let start = from + pos;
                let end = start + word.len();
                if sn.is_char_boundary(start) && sn.is_char_boundary(end) {
                    spans.push(Span::new(start, end).weight(500.0).color(tok.text));
                }
                from = end;
            }
        }
    }
    rich(sn.to_string(), spans)
        .font(S12)
        .color(tok.text2)
        .truncate()
        .mt(4.0)
}

fn skel_row(i: usize, (tw, th): (f32, f32)) -> El {
    let tok = t();
    let bar = |w: f32, h: f32| el().w(w).h(h).radius(4.0).fill(tok.skeleton);
    row()
        .key(format!("skelrow-{i}"))
        .w_full()
        .h(th)
        .gap(16.0)
        .child(el().w(tw).h(th).radius(12.0).fill(tok.skeleton))
        .child(
            col()
                .grow(1.0)
                .gap(10.0)
                .child(bar(320.0, 20.0))
                .child(bar(180.0, 14.0))
                .child(bar(120.0, 14.0)),
        )
}

// ----- states -------------------------------------------------------------------

/// An empty or error state: a 120 px icon, a 20/28 700 title, a line of secondary text, and an
/// optional action.
fn state(ic: &'static Icon, title: &str, body: &str, action: Option<El>) -> El {
    let tok = t();
    let mut c = col()
        .key(format!("state-{title}"))
        .w_full()
        .h_full()
        .center()
        .gap(12.0)
        .child(icon(ic, 120.0).color(tok.text2))
        .child(text(title.to_string()).font(S20B).color(tok.text))
        .child(
            text(body.to_string())
                .font(S14)
                .color(tok.text2)
                .wrap()
                .max_lines(3),
        );
    if let Some(a) = action {
        c = c.child(el().mt(12.0).child(a));
    }
    c
}

fn error_state(msg: &str) -> El {
    let ic = if msg.starts_with("Couldn't reach") {
        &OFFLINE
    } else {
        &ERROR
    };
    state(ic, "Something went wrong", msg, Some(retry_button()))
}

fn status(page: &Page, st: St) -> El {
    let tok = t();
    let k = kit();
    let msg = |l: &Load| match l {
        Load::Error(e) => e.clone(),
        _ => String::new(),
    };
    match st {
        St::MoreLoading | St::CommentsLoading => col()
            .key(format!("status-{st:?}"))
            .w_full()
            .h_full()
            .center()
            .child(Spinner::new().size(32.0).color(tok.text2).build(k)),
        St::MoreError | St::CommentsError => {
            let m = if st == St::MoreError {
                msg(&page.feed.more)
            } else {
                page.watch()
                    .map(|w| msg(&w.comments_state))
                    .unwrap_or_default()
            };
            row()
                .key(format!("status-{st:?}"))
                .w_full()
                .h_full()
                .center()
                .gap(16.0)
                .child(text(m).font(S14).color(tok.text2).truncate().shrink())
                .child(retry_button())
        }
        St::Error => {
            let m = page
                .watch()
                .and_then(|w| w.error.clone())
                .unwrap_or_else(|| msg(&page.feed.status));
            error_state(&m)
        }
        St::Empty => {
            let home = || {
                pill("Browse videos", None, true)
                    .key("go-home")
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.root(Kind::Home { topic: 0 }, tx))
            };
            match page.kind {
                Kind::Subs { .. } => state(
                    &SUBS,
                    "No subscriptions yet",
                    "Subscribe to channels to see their newest videos here.",
                    Some(home()),
                ),
                Kind::History => state(
                    &HISTORY,
                    "No watch history",
                    "Videos you watch will show up here.",
                    Some(home()),
                ),
                Kind::Later => state(
                    &LATER,
                    "Nothing saved for later",
                    "Use Watch Later on a video to save it here.",
                    Some(home()),
                ),
                Kind::Search { .. } => state(
                    &SEARCH,
                    "No results found",
                    "Try different keywords or remove search filters.",
                    None,
                ),
                _ => state(
                    &LIBRARY,
                    "Nothing here yet",
                    "There are no videos to show.",
                    None,
                ),
            }
        }
    }
}

// ----- channel, subscriptions, library -----------------------------------------------

fn banner(s: &App, page: &Page, w: f32, scale: f32) -> El {
    let url = match &page.kind {
        Kind::Channel { info: Some(i), .. } => i.banner.clone().unwrap_or_default(),
        _ => String::new(),
    };
    el().key("banner")
        .w_full()
        .h_full()
        .radius(16.0)
        .fill(t().skeleton)
        .pixels_fit(&s.images.get(&url, px(w, scale)), SurfaceFit::Fill)
        .clip_rounded(16.0)
}

fn channel_head(s: &App, page: &Page, scale: f32) -> El {
    let tok = t();
    let Kind::Channel {
        info: Some(info),
        desc_open,
        ..
    } = &page.kind
    else {
        return el();
    };
    let small = s.win_w < 600.0;
    let line = [
        info.handle.clone(),
        info.subscribers.clone(),
        info.video_count.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" • ");
    let toggle = |s: &mut App| {
        let p = s.top_mut();
        if let Kind::Channel { desc_open, .. } = &mut p.kind {
            *desc_open = !*desc_open;
        }
        p.touch();
    };
    let mut text_col = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(4.0)
        .child(
            text(info.name.clone())
                .font(S36B)
                .color(tok.text)
                .wrap()
                .max_lines(2),
        )
        .child(text(line).font(S14).color(tok.text2).truncate());
    if !info.description.is_empty() {
        let desc = if *desc_open {
            col()
                .w_full()
                .child(
                    text(info.description.clone())
                        .font(S14)
                        .color(tok.text2)
                        .wrap(),
                )
                .child(
                    text("Show less")
                        .font(S14M)
                        .color(tok.text)
                        .cursor(Cursor::Pointer)
                        .on_press(toggle),
                )
        } else {
            let first = info.description.lines().next().unwrap_or("").to_string();
            row()
                .w_full()
                .gap(4.0)
                .cursor(Cursor::Pointer)
                .on_press(toggle)
                .child(
                    text(first)
                        .font(S14)
                        .color(tok.text2)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                )
                .child(text("…more").font(S14M).color(tok.text))
        };
        text_col = text_col.child(desc);
    }
    text_col = text_col.child(el().mt(8.0).child(subscribe_button(info.as_ref())));
    row()
        .key("channel-head")
        .w_full()
        .h_full()
        .gap(24.0)
        .child(avatar(
            s,
            info.avatar.as_deref(),
            if small { 72.0 } else { 160.0 },
            scale,
        ))
        .child(text_col)
}

fn tabs() -> El {
    let tok = t();
    el().key("tabs")
        .w_full()
        .h_full()
        .border_sides([0.0, 0.0, 0.0, 1.0], tok.divider)
        .child(
            col()
                .h_full()
                .w(80.0)
                .center()
                .child(text("Videos").font(S16M).color(tok.text))
                .child(el().absolute(0.0, 46.0).w(80.0).h(2.0).fill(tok.text)),
        )
}

fn strip(s: &App, page: &Page, scale: f32) -> El {
    let tok = t();
    let Kind::Subs { channels } = &page.kind else {
        return el();
    };
    let mut r = row()
        .key("strip")
        .w_full()
        .h_full()
        .gap(16.0)
        .clip_soft(0.0);
    for (i, c) in channels.iter().enumerate() {
        let id = c.id.clone();
        r = r.child(focus(
            col()
                .key(format!("sub-{i}"))
                .w(96.0)
                .gap(8.0)
                .py(8.0)
                .items_center()
                .radius(12.0)
                .role("link")
                .label(c.name.clone())
                .cursor(Cursor::Pointer)
                .hover(tok.fill)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_channel(id.clone(), tx))
                .child(avatar(s, c.avatar.as_deref(), 64.0, scale))
                .child(text(c.name.clone()).font(S12).color(tok.text).truncate()),
        ));
    }
    r
}

fn lib_head(page: &Page) -> El {
    let tok = t();
    let title = if matches!(page.kind, Kind::History) {
        "History"
    } else {
        "Watch Later"
    };
    let mut r = row()
        .key("lib-head")
        .w_full()
        .h_full()
        .center_y()
        .justify_between()
        .child(text(title).font(S20B).color(tok.text));
    if matches!(page.kind, Kind::History) && !page.feed.items.is_empty() {
        r = r.child(
            pill("Clear all watch history", Some(&DELETE), false)
                .key("clear-history")
                .on_press(|s: &mut App| {
                    library().clear_history();
                    s.refresh_library_page();
                }),
        );
    }
    r
}
