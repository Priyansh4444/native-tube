//! The view: top bar, navigation rail, chip bar, the page's scroller (only mounted blocks are
//! built), the search dropdown, menus and toasts, and every feed block. The watch page's blocks
//! are in `watch.rs`.

use crate::app::{App, CardMenu};
use crate::pages::*;
use crate::probe::target;
use crate::scroller::BAR_W;
use crate::theme::*;
use crate::watch;
use nt_core::{SearchNotice, SearchOption, SearchResult, Video, library};
use rune::{
    Color, Cursor, Cx, El, Icon, Link, LinkPaint, Phase, PointerButton, Span, SurfaceFit, Tx, col,
    el, icon, rich, row, text,
};
use rune_kit::{Spinner, TextInput};
use std::sync::OnceLock;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    if crate::probe::enabled() {
        s.scene_version.set(s.scene_version.get() + 1);
    }
    cx.focus_ring(2.0, 2.0, t().text);
    let scale = cx.env().scale;
    s.scale.set(scale);
    let tok = t();
    let mut root = col()
        .key("root")
        .instant_changes()
        .w_full()
        .h_full()
        .fill(if s.native_picture() {
            Color::TRANSPARENT
        } else if s.fullscreen {
            Color::BLACK
        } else {
            tok.bg
        })
        .on_layout(|s: &mut App, r: [f32; 4]| {
            s.win_w = r[2];
            s.win_h = r[3];
            // The view this report runs again draws at the laid-out size and the window's scale.
            s.images.set_ready();
        })
        .on_wheel(|s: &mut App, e, tx| s.on_wheel(e, tx))
        .pointer_buttons([PointerButton::Back, PointerButton::Forward])
        .on_press_tx(|s: &mut App, tx: &mut Tx| match tx.button() {
            PointerButton::Back => s.back(tx),
            PointerButton::Forward => s.go_forward(tx),
            _ => {}
        });
    #[cfg(target_os = "linux")]
    if s.native_picture() {
        root = root.child(s.native_video.backdrop(s.fullscreen));
    }
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
        if s.guide_wide() {
            body = body.child(guide(s, scale, false));
        } else if page.has_rail(s.win_w) {
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
    if let Some(at) = s.player_menu {
        root = root.child(watch::player_menu(s, at));
    }
    if let Some(m) = &s.card_menu {
        root = root.child(card_menu(s, m));
    }
    if s.guide_over {
        root = root.child(guide_layer(s, scale));
    }
    if s.queue_open {
        root = root.child(queue_popup(s, scale));
    }
    if s.search_filters_open {
        root = root.child(search_filters(s));
    }
    if s.shortcuts {
        root = root.child(shortcuts_sheet(s));
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

fn retry_button(action: fn(&mut App)) -> El {
    pill("Retry", None, false).key("retry").on_press(action)
}

/// The duration badge (or LIVE), inset 8 from the thumbnail's bottom-right.
pub fn verified_name(name: &str, verified: bool) -> String {
    if verified {
        format!("{name} ✓")
    } else {
        name.to_string()
    }
}

/// Views • age, as core words them (`format::published_label`), or the upcoming line.
pub fn video_meta(v: &Video) -> String {
    match &v.upcoming {
        Some(upcoming) => upcoming.clone(),
        None => v.meta_line(),
    }
}

pub fn badges(v: &Video) -> El {
    let mut list = row().gap(4.0);
    for badge in v
        .badges
        .iter()
        .filter(|badge| !badge.eq_ignore_ascii_case("live"))
    {
        list = list.child(
            el().px(4.0)
                .h(16.0)
                .radius(2.0)
                .center_y()
                .fill(t().fill)
                .child(text(badge.clone()).font(S10).color(t().text2)),
        );
    }
    list
}

fn badge(v: &Video) -> Option<El> {
    if v.upcoming.is_some() {
        return None;
    }
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
pub fn player_rect(s: &App) -> Option<[f32; 4]> {
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
pub fn content_to_window(s: &App, x: f32, y: f32) -> [f32; 2] {
    let page = s.top();
    let left = s.side_w();
    let top = TOP_BAR + if page.has_chips() { CHIP_BAR } else { 0.0 };
    [left + x, top + y + page.scroll.live.get()[1]]
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
    let side = top_side(s.win_w);
    let left = row()
        .key("brand")
        .w(side)
        .gap(8.0)
        .center_y()
        .shrink()
        .child(
            icon_button("guide", &MENU, "Guide", tok.text).on_press(|s: &mut App| s.toggle_guide()),
        )
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

/// The top bar's left (and matching right) group width.
pub fn top_side(win_w: f32) -> f32 {
    if win_w < 900.0 { 210.0 } else { 240.0 }
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
        .build(input_kit())
        .focus_ring(2.0, 2.0, tok.text);
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
    let shown = nav_selected(s);
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

/// Which side-nav entry is selected: only on a root page (YouTube marks none on Search, Watch
/// or Channel).
pub fn nav_selected(s: &App) -> Option<usize> {
    if s.can_back() {
        return None;
    }
    match s.top().kind {
        Kind::Home { .. } => Some(0),
        Kind::Subs { .. } => Some(1),
        Kind::History => Some(2),
        Kind::Later => Some(3),
        _ => None,
    }
}

pub const GUIDE_ROW: f32 = 40.0;

/// The guide's rows: (key, icon, selected icon, label, page) for the four destinations.
type NavEntry = (&'static Icon, &'static Icon, &'static str, fn() -> Kind);
pub const NAV: [NavEntry; 4] = [
    (&HOME, &HOME_FILL, "Home", || Kind::Home { topic: 0 }),
    (&SUBS, &SUBS_FILL, "Subscriptions", || Kind::Subs {
        channels: Vec::new(),
    }),
    (&HISTORY, &HISTORY_FILL, "History", || Kind::History),
    (&LATER, &LATER_FILL, "Watch Later", || Kind::Later),
];

/// The full guide, 240 wide: Home and Subscriptions, then "You" (History, Watch Later), then
/// the subscribed channels; dividers between the sections. `over` is the copy drawn over a
/// watch page, with its own ☰ and brand on top.
fn guide(s: &App, scale: f32, over: bool) -> El {
    let tok = t();
    let sel = if over { None } else { nav_selected(s) };
    let entry = |i: usize| {
        let (ic, fill, label, kind) = NAV[i];
        let on = sel == Some(i);
        focus(
            row()
                .key(format!("guide-{i}"))
                .w_full()
                .h(GUIDE_ROW)
                .px(12.0)
                .gap(24.0)
                .center_y()
                .radius(10.0)
                .role("link")
                .label(label)
                .cursor(Cursor::Pointer)
                .fill(if on { tok.fill } else { Color::TRANSPARENT })
                .hover(tok.fill)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    s.guide_over = false;
                    s.root(kind(), tx);
                })
                .child(icon(if on { fill } else { ic }, 24.0).color(tok.text))
                .child(
                    text(label)
                        .font(if on { S14M } else { S14 })
                        .color(tok.text),
                ),
        )
    };
    let divider = || el().w_full().h(1.0).my(12.0).fill(tok.divider);
    let heading = |label: &'static str| {
        row()
            .w_full()
            .h(GUIDE_ROW)
            .px(12.0)
            .gap(8.0)
            .center_y()
            .child(text(label).font(S16M).color(tok.text))
    };
    let mut g = col()
        .key("guide")
        .w(GUIDE_W)
        .h_full()
        .px(12.0)
        .clip_soft(0.0);
    if over {
        g = g.child(
            row()
                .h(TOP_BAR)
                .gap(8.0)
                .ml(4.0)
                .center_y()
                .child(
                    icon_button("guide-close", &MENU, "Guide", tok.text)
                        .on_press(|s: &mut App| s.guide_over = false),
                )
                .child(text("NativeTube").font(S20B).color(tok.text).ml(8.0)),
        );
    } else {
        g = g.pt(12.0);
    }
    g = g
        .child(entry(0))
        .child(entry(1))
        .child(divider())
        .child(heading("You").child(icon(&CHEVRON_RIGHT, 20.0).color(tok.text)))
        .child(entry(2))
        .child(entry(3));
    g = g.child(target(
        pill("Queue", Some(&NEXT), false).on_press(|s: &mut App| {
            s.guide_over = false;
            s.queue_open = true;
        }),
        "queue",
    ));
    let subs = library().subscriptions();
    if !subs.is_empty() {
        g = g.child(divider()).child(heading("Subscriptions"));
        for (i, c) in subs.into_iter().take(12).enumerate() {
            let id = c.id.clone();
            g = g.child(focus(
                row()
                    .key(format!("guide-sub-{i}"))
                    .w_full()
                    .h(GUIDE_ROW)
                    .px(12.0)
                    .gap(24.0)
                    .center_y()
                    .radius(10.0)
                    .role("link")
                    .label(c.name.clone())
                    .cursor(Cursor::Pointer)
                    .hover(tok.fill)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        s.guide_over = false;
                        s.open_channel(id.clone(), tx);
                    })
                    .child(avatar(s, c.avatar.as_deref(), 24.0, scale))
                    .child(
                        text(c.name.clone())
                            .font(S14)
                            .color(tok.text)
                            .truncate()
                            .shrink()
                            .min_w(0.0),
                    ),
            ));
        }
    }
    g
}

/// The guide over a watch page (or a narrow window): a scrim that closes it, and the panel.
fn guide_layer(s: &App, scale: f32) -> El {
    el().key("guide-layer")
        .transition(rune::Transition::none())
        .instant_changes()
        .modal_tx(false, |s: &mut App, tx| {
            s.guide_over = false;
            tx.instant();
        })
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .child(
            el().key("guide-scrim")
                .absolute(0.0, 0.0)
                .w(s.win_w)
                .h(s.win_h)
                .fill(Color::BLACK.alpha(0.5))
                .on_press(|s: &mut App| s.guide_over = false)
                .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.guide_over = false),
        )
        .child(
            guide(s, scale, true)
                .absolute(0.0, 0.0)
                .h(s.win_h)
                .fill(t().bg)
                .on_press(|_: &mut App| {}),
        )
}

/// The `?` sheet: YouTube's keyboard shortcuts, in its groups. Escape or a click outside closes.
pub const SHORTCUTS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Playback",
        &[
            ("Toggle play/pause", "k or Space"),
            ("Rewind 10 seconds", "j"),
            ("Fast forward 10 seconds", "l"),
            ("Seek back / forward 5 seconds", "← / →"),
            ("Previous / next frame (paused)", ", / ."),
            ("Decrease / increase speed", "< / >"),
            ("Seek to 0%–90%", "0–9"),
            ("Beginning / end", "Home / End"),
        ],
    ),
    (
        "General",
        &[
            ("Toggle full screen", "f"),
            ("Toggle theatre mode", "t"),
            ("Toggle mute", "m"),
            ("Volume up / down", "↑ / ↓"),
            ("Captions on / off", "c"),
            ("Back / Forward", "Alt+← / Alt+→"),
        ],
    ),
    (
        "Search and help",
        &[
            ("Go to search", "/"),
            ("Keyboard shortcuts", "?"),
            ("Close a menu or sheet", "Escape"),
        ],
    ),
];

fn shortcuts_sheet(s: &App) -> El {
    let tok = t();
    let w = 560.0f32.min(s.win_w - 32.0);
    let mut sheet = col()
        .key("shortcuts")
        .role("dialog")
        .label("Keyboard shortcuts")
        .w(w)
        .max_h(s.win_h - 48.0)
        .padding(24.0)
        .gap(4.0)
        .radius(12.0)
        .fill(tok.menu)
        .shadow(4.0, 32.0, Color::BLACK.alpha(0.5))
        .clip_soft(0.0)
        .on_press(|_: &mut App| {})
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("Keyboard shortcuts").font(S20B).color(tok.text))
                .child(
                    icon_button("shortcuts-close", &CLOSE, "Close", tok.text)
                        .on_press(|s: &mut App| s.shortcuts = false),
                ),
        );
    for (group, keys) in SHORTCUTS {
        sheet = sheet.child(text(group).font(S16M).color(tok.text).mt(12.0).mb(4.0));
        for (what, key) in keys {
            sheet = sheet.child(
                row()
                    .w_full()
                    .h(28.0)
                    .center_y()
                    .justify_between()
                    .child(text(*what).font(S14).color(tok.text))
                    .child(text(*key).font(S14M).color(tok.text2)),
            );
        }
    }
    el().key("shortcuts-layer")
        .transition(rune::Transition::none())
        .instant_changes()
        .modal_tx(false, |s: &mut App, tx| {
            s.shortcuts = false;
            tx.instant();
        })
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .fill(Color::BLACK.alpha(0.5))
        .center()
        .on_press(|s: &mut App| s.shortcuts = false)
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.shortcuts = false)
        .child(sheet)
}

/// The chips' widths: text + 24 padding each, 12 apart.
pub fn chips_width(page: &Page) -> f32 {
    if !page.home_chips.is_empty() {
        return page
            .home_chips
            .iter()
            .map(|chip| rune::text::measure(&chip.label, &[], S14M, None).0 + 36.0)
            .sum::<f32>()
            - 12.0;
    }
    static W: OnceLock<f32> = OnceLock::new();
    *W.get_or_init(|| {
        nt_core::TOPICS
            .iter()
            .map(|t| rune::text::measure(t.title, &[], S14M, None).0 + 36.0)
            .sum::<f32>()
            - 12.0
    })
}

fn chip_bar(s: &App) -> El {
    let tok = t();
    let page = s.top();
    let cur = match page.kind {
        Kind::Home { topic } => topic,
        _ => 0,
    };
    let pad = s.geo().pad();
    let room = (chips_width(page) - (s.vw - 2.0 * pad)).max(0.0);
    let mut chips = row()
        .key("chips")
        .gap(12.0)
        .center_y()
        .live_offset(&s.chips_live);
    let server = !page.home_chips.is_empty();
    let titles: Vec<&str> = if server {
        page.home_chips
            .iter()
            .map(|chip| chip.label.as_str())
            .collect()
    } else {
        nt_core::TOPICS.iter().map(|topic| topic.title).collect()
    };
    for (i, title) in titles.into_iter().enumerate() {
        let on = if server {
            page.home_chip == Some(i)
        } else {
            i == cur
        };
        chips = chips.child(focus(
            el().key(format!("chip-{i}"))
                .h(32.0)
                .px(12.0)
                .radius(8.0)
                .center()
                .role("button")
                .label(title.to_string())
                .cursor(Cursor::Pointer)
                .fill(if on { tok.chip_sel_bg } else { tok.fill })
                .hover(if on { Color::TRANSPARENT } else { tok.fill })
                .on_press(move |s: &mut App| {
                    if server {
                        s.set_home_chip(i)
                    } else {
                        s.set_topic(i)
                    }
                })
                .child(text(title.to_string()).font(S14M).color(if on {
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
        list = list.child(target(
            row()
                .key(format!("sg-{i}"))
                .label(q.clone())
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
            "suggestion",
        ));
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
        .child(
            item("menu-share", &SHARE, "Share").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.share(&v2.id, tx);
            }),
        );
    let with_channel = v3.channel_id.as_deref().is_some_and(|c| !c.is_empty());
    if let Some(cid) = v3.channel_id.clone().filter(|_| with_channel) {
        menu = menu.child(item("menu-channel", &SUBS, "Go to channel").on_press_tx(
            move |s: &mut App, tx: &mut Tx| {
                s.card_menu = None;
                s.open_channel(cid.clone(), tx);
            },
        ));
    }
    let queued = m.video.clone();
    menu = menu.child(item("menu-queue", &NEXT, "Add to queue").on_press_tx(
        move |s: &mut App, tx: &mut Tx| {
            library().enqueue(&queued);
            s.card_menu = None;
            s.show_toast("Added to queue", tx);
        },
    ));
    let allow_hide = matches!(
        s.top().kind,
        Kind::Home { .. } | Kind::Search(_) | Kind::Watch(_)
    );
    if allow_hide {
        let video = m.video.clone();
        menu = menu.child(
            item("menu-not-interested", &CLOSE, "Not interested").on_press_tx(
                move |s: &mut App, tx: &mut Tx| s.hide_recommendation(&video, false, tx),
            ),
        );
        if with_channel {
            let video = m.video.clone();
            menu = menu.child(
                item("menu-hide-channel", &CLOSE, "Don’t recommend channel").on_press_tx(
                    move |s: &mut App, tx: &mut Tx| s.hide_recommendation(&video, true, tx),
                ),
            );
        }
    }
    let rows = 4
        + usize::from(with_channel)
        + if allow_hide {
            1 + usize::from(with_channel)
        } else {
            0
        };
    let h = 16.0 + 40.0 * rows as f32;
    let [x, y] = menu_origin(m, h, s.win_w, s.win_h);
    // A full-window catcher: a press (either button) anywhere else only closes the menu.
    el().key("menu-layer")
        .transition(rune::Transition::none())
        .instant_changes()
        .modal_tx(false, |s: &mut App, tx| {
            s.card_menu = None;
            tx.instant();
        })
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .on_press(|s: &mut App| s.card_menu = None)
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.card_menu = None)
        .child(menu.absolute(x, y))
}

fn queue_popup(s: &App, scale: f32) -> El {
    let tok = t();
    let dismiss = |s: &mut App| s.queue_open = false;
    let mut items = col().key("queue-list").w_full().gap(8.0);
    for (i, video) in library().queue().into_iter().enumerate() {
        let id = video.id.clone();
        let open = video.clone();
        items = items.child(
            row()
                .key(format!("queue-{i}"))
                .w_full()
                .gap(8.0)
                .center_y()
                .child(thumb(s, &video, 96.0, 54.0, 8.0, scale))
                .child(target(
                    focus(
                        text(video.title.clone())
                            .label(video.title)
                            .font(S14M)
                            .color(tok.text)
                            .grow(1.0)
                            .truncate()
                            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                                library().remove_queued(&open.id);
                                s.open_video(open.clone(), tx);
                            }),
                    ),
                    "row",
                ))
                .child(target(
                    icon_button("queue-remove", &CLOSE, "Remove", tok.text)
                        .on_press(move |_: &mut App| library().remove_queued(&id)),
                    "remove",
                )),
        );
    }
    let dialog = col()
        .w((s.win_w - 40.0).min(600.0))
        .max_h(s.win_h - 40.0)
        .padding(16.0)
        .gap(12.0)
        .radius(12.0)
        .fill(tok.menu)
        .on_press(|_: &mut App| {})
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("Queue").font(S20B).color(tok.text))
                .child(target(
                    icon_button("queue-close", &CLOSE, "Close", tok.text).on_press(dismiss),
                    "close",
                )),
        )
        .child(
            col()
                .w_full()
                .max_h(s.win_h - 128.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .clip_soft(0.0)
                .child(if library().queue().is_empty() {
                    text("Your queue is empty").font(S14).color(tok.text2)
                } else {
                    items
                }),
        );
    el().key("queue-layer")
        .absolute(0.0, 0.0)
        .size(s.win_w, s.win_h)
        .center()
        .fill(Color::BLACK.alpha(0.5))
        .instant_changes()
        .transition(rune::Transition::none())
        .modal(false, dismiss)
        .on_press(dismiss)
        .child(dialog)
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
        B::Resume => s
            .resume
            .as_ref()
            .map_or_else(el, |saved| resume_card(s, saved, scale)),
        B::Card(i) => items
            .get(i)
            .map_or_else(el, |v| card(s, v, i, b, plan.card_channel, scale)),
        B::SkelCard(i) => skel_card(i, b.w, plan.card_channel),
        B::Row(i) => items.get(i).map_or_else(el, |v| {
            let query = match &page.kind {
                Kind::Search(sr) => Some(sr.query.as_str()),
                _ => None,
            };
            list_row(s, v, i, b, plan.row_thumb, query, scale)
        }),
        B::TRow(i) => match &page.kind {
            Kind::Search(sr) => sr.trans.items.get(i).map_or_else(el, |v| {
                list_row(s, v, i, b, plan.row_thumb, Some(sr.query.as_str()), scale)
                    .key(format!("trow-{i}-{}", v.id))
            }),
            _ => el(),
        },
        B::SearchControls => match &page.kind {
            Kind::Search(search) => search_controls(search),
            _ => el(),
        },
        B::SearchEntry(i) => match &page.kind {
            Kind::Search(search) => search_entry(s, search, i, b, plan.row_thumb, scale),
            _ => el(),
        },
        B::Notice(i) => match &page.kind {
            Kind::Search(sr) => sr.notices.get(i).map_or_else(el, notice),
            _ => el(),
        },
        B::TransHead => trans_head(page),
        B::SkelRow(i) => skel_row(i, plan.row_thumb),
        B::Compact(i) => items
            .get(i)
            .map_or_else(el, |v| watch::compact(s, v, i, b, scale)),
        B::SkelCompact(i) => watch::skel_compact(i),
        B::Status(st) => status(page, st),
        B::Banner => banner(s, page, b.w, scale),
        B::ChannelHead => channel_head(s, page, scale),
        B::Tabs => tabs(page),
        B::ChannelSort => channel_sort(page),
        B::PlaylistCard(i) => match &page.kind {
            Kind::Channel {
                info: Some(info), ..
            } => info.playlists.items.get(i).map_or_else(el, |playlist| {
                let w = b.w;
                playlist_card(s, playlist, w, w * 9.0 / 16.0, scale, true)
            }),
            _ => el(),
        },
        B::PlaylistCardsMore => playlist_more(&page.playlist_cards_more, App::load_playlist_cards),
        B::PlaylistHeading => watch::playlist_heading(page),
        B::PlaylistRow(i) => watch::playlist_row(s, page, i, scale),
        B::PlaylistMore => page.watch().map_or_else(el, |w| {
            playlist_more(&w.playlist_more, App::load_playlist_more)
        }),
        B::Strip => strip(s, page, scale),
        B::LibHead => lib_head(page),
        B::Player => watch::player(s, b.w, b.h, false),
        B::Info => watch::info(s, page, b, scale),
        B::SkelInfo => watch::skel_info(b.w),
        B::Desc => watch::description(page, plan, b),
        B::Chapters => watch::chapters(s, page, scale),
        B::CommentsHead => watch::comments_head(page),
        B::RelatedChips => watch::related_chips(page),
        B::RepliesMore(i) => watch::replies_more(page, i),
        B::Reply(i, j) => page
            .watch()
            .and_then(|w| w.comments.get(i).and_then(|c| w.replies.get(&c.id)))
            .and_then(|r| r.items.get(j))
            .map_or_else(el, |c| watch::comment(s, page, c, j, b, scale, true)),
        B::Comment(i) => page
            .watch()
            .and_then(|w| w.comments.get(i))
            .map_or_else(el, |c| watch::comment(s, page, c, i, b, scale, false)),
    }
}

// ----- feed blocks --------------------------------------------------------------

fn resume_card(s: &App, saved: &nt_core::LastPlayed, scale: f32) -> El {
    let tok = t();
    let position = nt_core::format::clock(saved.position);
    let label = format!("Continue watching {} at {position}", saved.title);
    let thumbnail = Video::thumbnail_url(&saved.id);
    target(
        focus(
            row()
                .key("resume")
                .px(12.0)
                .gap(12.0)
                .center_y()
                .role("button")
                .label(label)
                .radius(12.0)
                .fill(tok.fill)
                .hover(tok.fill_hover)
                .cursor(Cursor::Pointer)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.resume_last(tx))
                .child(
                    el().size(112.0, 63.0)
                        .shrink()
                        .radius(8.0)
                        .fill(tok.skeleton)
                        .pixels_fit(
                            &s.images.get(&thumbnail, px(112.0, scale)),
                            SurfaceFit::Fill,
                        ),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(text("Continue watching").font(S16M).color(tok.text))
                        .child(
                            text(saved.title.clone())
                                .font(S14)
                                .color(tok.text)
                                .truncate_fade(),
                        )
                        .child(text(position).font(S12).color(tok.text2)),
                ),
        ),
        "resume",
    )
}

/// Opens the video; a transcript match opens at its moment.
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
    let mut picture = thumb(s, v, w, th, 12.0, scale);
    if let Some(preview) = s.preview.pixels(&v.id) {
        picture = picture.child(
            el().key("hover-preview")
                .absolute(0.0, 0.0)
                .size(w, th)
                .pixels_fit(preview, SurfaceFit::Fill)
                .inert(),
        );
    }
    let id = v.id.clone();
    let width = px(w, scale);
    if with_channel {
        if v.channel_avatar.is_some() {
            info = info.child(
                avatar(s, v.channel_avatar.as_deref(), 36.0, scale)
                    .cursor(Cursor::Pointer)
                    .on_press_tx(channel_handler(v)),
            );
        }
        text_col = text_col.child(
            text(verified_name(&v.channel, v.channel_verified))
                .font(S14)
                .color(tok.text2)
                .truncate()
                .on_press_tx(channel_handler(v)),
        );
    }
    text_col = text_col.child(
        row()
            .gap(6.0)
            .center_y()
            .child(
                text(video_meta(v))
                    .font(S14)
                    .color(tok.text2)
                    .truncate()
                    .shrink(),
            )
            .child(badges(v)),
    );
    info = info
        .child(text_col)
        .child(more_button(v, [b.x + w - 40.0, b.y + th + 12.0]).mr(-12.0));
    col()
        .key(format!("card-{i}-{}", v.id))
        .w(w)
        .hover_group()
        .on_secondary_press(context_menu(v))
        .on_hover_change(move |s: &mut App, on, tx| s.preview.hover(&id, width, on, tx))
        // YouTube's hover box: #272727, radius 12, 8 px past the card on every side, behind it.
        .child(
            el().key("tint")
                .absolute(-8.0, -8.0)
                .w(w + 16.0)
                .h(b.h + 16.0)
                .radius(12.0)
                .fill(tok.hover_tint)
                .group_opacity(0.0, 1.0)
                .inert(),
        )
        .child(target(
            focus(
                picture
                    .role("link")
                    .label(v.title.clone())
                    .cursor(Cursor::Pointer)
                    .press(Color::BLACK.alpha(0.1))
                    .on_press_tx(open_handler(v)),
            ),
            "card",
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
        text(verified_name(&v.channel, v.channel_verified))
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
        .child(text(video_meta(v)).font(S12).color(tok.text2).truncate())
        .child(chan)
        .child(badges(v));
    if let Some(m) = &v.moment {
        info = info.child(moment_line(m));
    } else if let Some(sn) = &v.snippet {
        info = info.child(snippet(sn, query));
    }
    row()
        .key(format!("row-{i}-{}", v.id))
        .w_full()
        .h(th)
        .gap(16.0)
        .hover_group()
        .on_secondary_press(context_menu(v))
        .child(target(
            focus(
                thumb(s, v, tw, th, 12.0, scale)
                    .role("link")
                    .label(v.title.clone())
                    .cursor(Cursor::Pointer)
                    .press(Color::BLACK.alpha(0.1))
                    .on_press_tx(open_handler(v)),
            ),
            "row",
        ))
        .child(info)
}

fn filter_option(option: &SearchOption) -> El {
    let button = pill(&option.label, None, option.selected);
    if let Some(filter) = &option.filter {
        let filter = filter.clone();
        target(
            button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.search_filter(filter.clone(), tx);
            }),
            "chip",
        )
    } else {
        button.focusable(false).opacity(0.5).inert()
    }
}

fn search_controls(search: &Search) -> El {
    let mut chips = row()
        .h_full()
        .gap(8.0)
        .center_y()
        .grow(1.0)
        .min_w(0.0)
        .clip_soft(0.0);
    for option in &search.chips {
        if option.label.eq_ignore_ascii_case("shorts") {
            continue;
        }
        chips = chips.child(filter_option(option));
    }
    let mut toolbar = row().w_full().h_full().gap(8.0).center_y().child(chips);
    if !search.filters.is_empty() {
        toolbar = toolbar.child(target(
            pill("Filters", Some(&SETTINGS), false)
                .key("search-filters-toggle")
                .on_press(|s: &mut App| s.search_filters_open = true),
            "filters",
        ));
    }
    toolbar
}

fn search_filters(s: &App) -> El {
    let Kind::Search(search) = &s.top().kind else {
        return el();
    };
    let tok = t();
    let mut options = row().w_full().gap(20.0).flex_wrap();
    for group in &search.filters {
        let mut column = col()
            .w(150.0)
            .gap(4.0)
            .child(text(group.title.clone()).font(S14M).color(tok.text).mb(8.0));
        for option in &group.options {
            if !option.label.eq_ignore_ascii_case("shorts") {
                column = column.child(filter_option(option));
            }
        }
        options = options.child(column);
    }
    let dismiss = |s: &mut App| s.search_filters_open = false;
    let dialog = col()
        .key("search-filters")
        .role("dialog")
        .label("Search filters")
        .w((s.win_w - 48.0).min(880.0))
        .max_h(s.win_h - 48.0)
        .padding(24.0)
        .gap(16.0)
        .radius(12.0)
        .fill(tok.menu)
        .on_press(|_: &mut App| {})
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("Search filters").font(S20B).color(tok.text))
                .child(target(
                    icon_button("filters-close", &CLOSE, "Close", tok.text).on_press(dismiss),
                    "close",
                )),
        )
        .child(
            col()
                .w_full()
                .max_h(s.win_h - 160.0)
                .clip_soft(0.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(options),
        );
    el().key("search-filters-layer")
        .absolute(0.0, 0.0)
        .size(s.win_w, s.win_h)
        .instant_changes()
        .transition(rune::Transition::none())
        .modal(false, dismiss)
        .on_press(dismiss)
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.search_filters_open = false)
        .center()
        .fill(Color::BLACK.alpha(0.5))
        .child(dialog)
}

fn search_entry(
    s: &App,
    search: &Search,
    i: usize,
    b: &Block,
    thumb_size: (f32, f32),
    scale: f32,
) -> El {
    let tok = t();
    let Some(entry) = search.entries.get(i) else {
        return el();
    };
    match entry {
        SearchResult::Video(v) => list_row(s, v, i, b, thumb_size, Some(&search.query), scale),
        SearchResult::Shelf(shelf) => el()
            .w_full()
            .h_full()
            .center_y()
            .child(text(shelf.title.clone()).font(S20B).color(tok.text)),
        SearchResult::Channel(found) => {
            let channel = &found.channel;
            let id = channel.id.clone();
            let name = verified_name(&channel.name, found.verified);
            let metadata = [found.handle.as_deref(), channel.subscribers.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            target(
                focus(
                    row()
                        .key(format!("result-channel-{i}"))
                        .w_full()
                        .h(b.h)
                        .center_y()
                        .gap(16.0)
                        .radius(12.0)
                        .hover(tok.fill)
                        .role("link")
                        .label(name.clone())
                        .cursor(Cursor::Pointer)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_channel(id.clone(), tx))
                        .child(el().w(thumb_size.0).h_full().center().child(avatar(
                            s,
                            channel.avatar.as_deref(),
                            b.h.min(112.0),
                            scale,
                        )))
                        .child(
                            col()
                                .gap(6.0)
                                .min_w(0.0)
                                .child(text(name).font(S18).color(tok.text).truncate())
                                .child(text(metadata).font(S14).color(tok.text2).truncate()),
                        ),
                ),
                "channel",
            )
        }
        SearchResult::Playlist(playlist) => {
            playlist_card(s, playlist, thumb_size.0, thumb_size.1, scale, false)
        }
    }
}

fn playlist_card(
    s: &App,
    playlist: &nt_core::PlaylistRef,
    w: f32,
    h: f32,
    scale: f32,
    vertical: bool,
) -> El {
    let tok = t();
    let count = playlist.video_count.as_deref().unwrap_or("Playlist");
    let mut cover = el()
        .absolute(0.0, 8.0)
        .size(w, h - 8.0)
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(tok.skeleton);
    if let Some(url) = &playlist.thumbnail {
        cover = cover.pixels_fit(&s.images.get(url, px(w, scale)), SurfaceFit::Fill);
    }
    let stack = el()
        .size(w, h)
        .radius(12.0)
        .child(
            el().absolute(12.0, 0.0)
                .w(w - 24.0)
                .h(16.0)
                .radius(8.0)
                .fill(tok.fill),
        )
        .child(cover)
        .child(
            el().absolute(8.0, h - 28.0)
                .px(6.0)
                .py(3.0)
                .radius(4.0)
                .fill(Color::BLACK.alpha(0.8))
                .child(text(count.to_string()).font(S12M).color(Color::WHITE)),
        );
    let id = playlist.id.clone();
    let base = if vertical { col() } else { row() };
    target(
        focus(
            base.key(format!("playlist-{id}"))
                .w_full()
                .h_full()
                .gap(12.0)
                .role("link")
                .label(playlist.title.clone())
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| s.open_playlist(id.clone()))
                .child(stack)
                .child(
                    col()
                        .min_w(0.0)
                        .gap(4.0)
                        .child(
                            text(playlist.title.clone())
                                .font(S16M)
                                .color(tok.text)
                                .max_lines(2),
                        )
                        .child(text("Playlist").font(S14).color(tok.text2)),
                ),
        ),
        "playlist",
    )
}

fn playlist_more(state: &Load, request: fn(&mut App)) -> El {
    match state {
        Load::Loading => el()
            .w_full()
            .h_full()
            .center()
            .child(Spinner::new().size(22.0).build(kit())),
        Load::Error(_) => row()
            .w_full()
            .h_full()
            .center()
            .gap(12.0)
            .child(text("Couldn't load more").font(S14).color(t().text2))
            .child(pill("Retry", None, false).key("retry").on_press(request)),
        Load::Idle => el(),
    }
}

/// A search notice in YouTube's wording; corrections and suggestions are search links.
fn notice(n: &SearchNotice) -> El {
    let tok = t();
    let link_text = |prefix: &str, term: &str, style: rune::TextStyle| {
        let content = format!("{prefix}{term}");
        let r = prefix.len()..content.len();
        rich(
            content,
            vec![
                Span::new(r.start, r.end)
                    .color(tok.link)
                    .weight(500.0)
                    .italic(),
            ],
        )
        .font(style)
        .color(tok.text)
        .link(Link::new(r, format!("q:{term}")).hover(LinkPaint::new().underline(tok.link)))
        .on_link(|s: &mut App, id: &str, tx: &mut Tx| {
            if let Some(q) = id.strip_prefix("q:") {
                s.search.replace_text(q.to_string());
                s.submit_search(tx);
            }
        })
    };
    let c = col().key("notice").w_full().h_full().gap(4.0);
    match n {
        SearchNotice::AgeGated { title, body } => c
            .gap(8.0)
            .child(text(title.clone()).font(S20B).color(tok.text).wrap())
            .child(text(body.clone()).font(S14).color(tok.text2).wrap()),
        SearchNotice::Corrected { showing, original } => c
            .child(link_text("Showing results for ", showing, S16))
            .child(link_text("Search instead for ", original, S14)),
        SearchNotice::DidYouMean { suggestion } => {
            c.child(link_text("Did you mean: ", suggestion, S16))
        }
        SearchNotice::NoResults => c
            .child(text("No results found").font(S20B).color(tok.text))
            .child(
                text("Try different keywords or remove search filters.")
                    .font(S14)
                    .color(tok.text2)
                    .wrap(),
            ),
    }
}

/// The transcript section's heading.
fn trans_head(page: &Page) -> El {
    let tok = t();
    let total = match &page.kind {
        Kind::Search(sr) => sr.trans_total,
        _ => None,
    };
    let mut r = row()
        .key("trans-head")
        .w_full()
        .h_full()
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 1.0, 0.0, 0.0], tok.divider)
        .child(
            text("Matches in video transcripts · search.pronsh.dev")
                .font(S16M)
                .color(tok.text),
        );
    if let Some(n) = total {
        r = r.child(
            text(format!("{n} {}", if n == 1 { "match" } else { "matches" }))
                .font(S14)
                .color(tok.text2),
        );
    }
    r
}

/// A transcript match: the "▶ 9:52" chip, then the matched line with its highlights bold.
fn moment_line(m: &nt_core::Moment) -> El {
    let tok = t();
    let mut spans = Vec::new();
    for r in &m.highlights {
        if r.start < r.end
            && m.snippet.is_char_boundary(r.start)
            && m.snippet.is_char_boundary(r.end)
            && r.end <= m.snippet.len()
        {
            spans.push(Span::new(r.start, r.end).weight(700.0).color(tok.text));
        }
    }
    let label = if m.label.is_empty() {
        nt_core::format::clock(m.start_secs)
    } else {
        m.label.clone()
    };
    row()
        .key("moment")
        .w_full()
        .mt(8.0)
        .gap(8.0)
        .child(
            row()
                .key("moment-chip")
                .h(24.0)
                .px(8.0)
                .gap(4.0)
                .radius(4.0)
                .center_y()
                .fill(tok.fill)
                .child(icon(&PLAY, 16.0).color(tok.text))
                .child(text(label).font(S12M).color(tok.text)),
        )
        .child(
            rich(m.snippet.clone(), spans)
                .font(S14)
                .color(tok.text2)
                .wrap()
                .max_lines(2)
                .grow(1.0)
                .min_w(0.0),
        )
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
    state(
        ic,
        "Something went wrong",
        msg,
        Some(retry_button(App::load_top)),
    )
}

fn status(page: &Page, st: St) -> El {
    let tok = t();
    let k = kit();
    let msg = |l: &Load| match l {
        Load::Error(e) => e.clone(),
        _ => String::new(),
    };
    match st {
        St::MoreLoading | St::CommentsLoading | St::TransLoading => col()
            .key(format!("status-{st:?}"))
            .w_full()
            .h_full()
            .center()
            .child(Spinner::new().size(32.0).color(tok.text2).build(k)),
        St::MoreError | St::CommentsError | St::TransError => {
            let (m, action): (_, fn(&mut App)) = match st {
                St::MoreError => (msg(&page.feed.more), App::retry_more),
                St::TransError => (
                    match &page.kind {
                        Kind::Search(sr) => msg(&sr.trans.more),
                        _ => String::new(),
                    },
                    App::retry_transcripts,
                ),
                _ => (
                    page.watch()
                        .map(|w| msg(&w.comments_state))
                        .unwrap_or_default(),
                    App::load_comments,
                ),
            };
            row()
                .key(format!("status-{st:?}"))
                .w_full()
                .h_full()
                .center()
                .gap(16.0)
                .child(
                    text(format!("Couldn't load more · {m}"))
                        .font(S14)
                        .color(tok.text2)
                        .truncate()
                        .shrink(),
                )
                .child(retry_button(action))
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
    let toggle = |s: &mut App, tx: &mut Tx| s.channel_about(tx);
    let mut text_col = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(4.0)
        .child(
            text(verified_name(&info.name, info.verified))
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
                        .on_press_tx(toggle),
                )
        } else {
            let first = info.description.lines().next().unwrap_or("").to_string();
            row()
                .w_full()
                .gap(4.0)
                .cursor(Cursor::Pointer)
                .on_press_tx(toggle)
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
    text_col = text_col.child(
        el().mt(8.0)
            .child(subscribe_button(nt_core::ChannelInfo::as_ref(info))),
    );
    if *desc_open {
        for link in &info.links {
            let url = link.url.clone();
            text_col = text_col.child(focus(
                text(link.label.clone())
                    .h(28.0)
                    .font(S14M)
                    .color(tok.link)
                    .role("link")
                    .label(link.label.clone())
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |_: &mut App, tx: &mut Tx| {
                        if url.starts_with("https://") || url.starts_with("http://") {
                            let _ = tx.open_url(&url);
                        }
                    }),
            ));
        }
    } else if info.links_label.is_some() || info.about.is_some() {
        text_col = text_col.child(focus(
            text(info.links_label.as_deref().unwrap_or("About").to_string())
                .h(28.0)
                .font(S14M)
                .color(tok.link)
                .cursor(Cursor::Pointer)
                .on_press_tx(toggle),
        ));
    }
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

fn tabs(page: &Page) -> El {
    use nt_core::ChannelTab::*;
    let tok = t();
    let Kind::Channel {
        info: Some(info), ..
    } = &page.kind
    else {
        return el();
    };
    let available = if info.tabs.is_empty() {
        &[Videos][..]
    } else {
        &info.tabs
    };
    let selected = page.channel_tab.unwrap_or(Videos);
    let mut bar = row()
        .w_full()
        .h_full()
        .gap(16.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], tok.divider);
    for tab in available.iter().copied() {
        let label = match tab {
            Home => "Home",
            Videos => "Videos",
            Live => "Live",
            Playlists => "Playlists",
        };
        bar = bar.child(target(
            focus(
                el().px(10.0)
                    .h_full()
                    .center()
                    .role("tab")
                    .label(label)
                    .cursor(Cursor::Pointer)
                    .border_sides(
                        [0.0, 0.0, if tab == selected { 2.0 } else { 0.0 }, 0.0],
                        tok.text,
                    )
                    .child(text(label).font(S16M).color(if tab == selected {
                        tok.text
                    } else {
                        tok.text2
                    }))
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.channel_tab(tab, tx)),
            ),
            "tab",
        ));
    }
    bar
}

fn channel_sort(page: &Page) -> El {
    let Kind::Channel {
        info: Some(info), ..
    } = &page.kind
    else {
        return el();
    };
    let mut bar = row().h_full().gap(8.0).center_y();
    for (i, chip) in info.video_sort.iter().enumerate() {
        let selected = page.channel_sort.map_or(chip.selected, |index| index == i);
        bar = bar.child(target(
            pill(&chip.label, None, selected).on_press(move |s: &mut App| s.sort_channel(i)),
            "chip",
        ));
    }
    bar
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
