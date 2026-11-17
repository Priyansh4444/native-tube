//! The view: top bar, navigation rail, the page's scroller (only mounted blocks are built),
//! the search dropdown, and every block's elements.

use crate::app::App;
use crate::pages::*;
use crate::scroller::{BAR_W, Scroller};
use crate::theme::*;
use crate::video;
use nt_core::{Video, library};
use rune::{Color, Cx, El, Icon, Phase, PointerButton, SurfaceFit, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Spinner, TextInput};

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let scale = cx.env().scale;
    s.scale.set(scale);
    let size = cx.layout_size("viewport").unwrap_or([s.vw, s.vh]);
    let tok = t();
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(tok.bg)
        .on_wheel(|s: &mut App, e, tx| s.on_wheel(e, tx))
        .pointer_buttons([PointerButton::Back])
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            if tx.button() == PointerButton::Back {
                s.back();
            }
        })
        .child(top_bar(s))
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(rail(s))
                .child(viewport(s, size, scale)),
        );
    if s.sugg_open
        && let Some(r) = cx.layout("search-box")
    {
        root = root.child(dropdown(s, r));
    }
    s.images.end_view();
    root
}

// ----- chrome -----------------------------------------------------------------

fn top_bar(s: &App) -> El {
    let tok = t();
    let can_back = s.pages.len() > 1;
    let back = el()
        .key("back")
        .size(40.0, 40.0)
        .radius(20.0)
        .center()
        .role("button")
        .label("Back")
        .child(icon(&BACK, 22.0).color(if can_back { tok.text } else { tok.hover }));
    let back = if can_back {
        back.hover(tok.wash).on_press(|s: &mut App| s.back())
    } else {
        back
    };
    let field = TextInput::new(&s.search)
        .placeholder("Search")
        .label("Search")
        .width(None)
        .height(40.0)
        .text_style(rune::TextStyle::new(16.0, 400.0).lh(22.0))
        .on_edit(|s: &mut App| &mut s.search)
        .on_change(|s: &mut App, tx: &mut Tx| s.search_changed(tx))
        .on_key(|s: &mut App, ev, tx| s.search_key(ev, tx))
        .on_submit(|s: &mut App, tx: &mut Tx| s.submit_search(tx))
        .build(input_kit());
    let search_btn = el()
        .key("search-go")
        .size(64.0, 40.0)
        .radius(20.0)
        .fill(tok.raised)
        .hover(tok.wash)
        .center()
        .role("button")
        .label("Search")
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.submit_search(tx))
        .child(icon(&SEARCH, 20.0).color(tok.text));
    let search = row()
        .key("search-box")
        .grow(1.0)
        .max_w(640.0)
        .min_w(120.0)
        .gap(8.0)
        .center_y()
        .child(el().key("search").grow(1.0).min_w(60.0).child(field))
        .child(search_btn);
    row()
        .key("topbar")
        .w_full()
        .h(TOP_BAR)
        .px(16.0)
        .gap(16.0)
        .center_y()
        .fill(tok.bg)
        .child(
            row()
                .w(170.0)
                .gap(12.0)
                .center_y()
                .child(back)
                .child(text("NativeTube").font(S18.lh(24.0)).color(tok.text)),
        )
        .child(row().grow(1.0).min_w(0.0).justify_center().child(search))
        .child(el().w(40.0))
}

fn rail(s: &App) -> El {
    let tok = t();
    let current = match s.pages.first().map(|p| &p.kind) {
        Some(Kind::Home { .. }) => 0,
        Some(Kind::Subs { .. }) => 1,
        Some(Kind::History) => 2,
        Some(Kind::Later) => 3,
        _ => 0,
    };
    let item = |i: usize, ic: &'static Icon, label: &'static str, go: fn(&mut App)| {
        let on = i == current;
        col()
            .key(format!("rail-{i}"))
            .w(64.0)
            .h(68.0)
            .radius(10.0)
            .center()
            .gap(4.0)
            .role("button")
            .label(label)
            .fill(if on { tok.raised } else { Color::TRANSPARENT })
            .hover(tok.wash)
            .on_press(go)
            .child(icon(ic, 22.0).color(if on { tok.text } else { tok.text2 }))
            .child(
                text(label)
                    .font(rune::TextStyle::new(10.0, if on { 600.0 } else { 400.0 }))
                    .color(if on { tok.text } else { tok.text2 }),
            )
    };
    col()
        .key("rail")
        .w(RAIL)
        .h_full()
        .pt(4.0)
        .gap(4.0)
        .items_center()
        .child(item(0, &HOME, "Home", |s| s.root(Kind::Home { topic: 0 })))
        .child(item(1, &SUBS, "Subscriptions", |s| {
            s.root(Kind::Subs {
                channels: Vec::new(),
            })
        }))
        .child(item(2, &HISTORY, "History", |s| s.root(Kind::History)))
        .child(item(3, &LATER, "Watch Later", |s| s.root(Kind::Later)))
}

fn dropdown(s: &App, r: [f32; 4]) -> El {
    let tok = t();
    let mut list = col()
        .key("suggest")
        .absolute(r[0], r[1] + r[3] + 4.0)
        .w(r[2] - 72.0)
        .py(8.0)
        .radius(12.0)
        .fill(tok.raised)
        .shadow(4.0, 16.0, Color::BLACK.alpha(0.5));
    for (i, q) in s.sugg.iter().enumerate() {
        let sel = s.sugg_sel == Some(i);
        list = list.child(
            row()
                .key(format!("sg-{i}"))
                .w_full()
                .h(36.0)
                .px(16.0)
                .gap(12.0)
                .center_y()
                .fill(if sel { tok.hover } else { Color::TRANSPARENT })
                .hover(tok.wash)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.choose_suggestion(i, tx))
                .child(icon(&SEARCH, 16.0).color(tok.text2))
                .child(
                    text(q.clone())
                        .font(S16M.lh(22.0))
                        .color(tok.text)
                        .truncate(),
                ),
        );
    }
    list
}

// ----- the scroller -----------------------------------------------------------

fn viewport(s: &App, size: [f32; 2], scale: f32) -> El {
    let page = s.top();
    let [vw, vh] = size;
    let plan = page.plan(vw, vh + TOP_BAR);
    let (top, bottom) = page.scroll.mounted();
    let mut content = el()
        .key(format!("content-{}", page.id))
        .w_full()
        .h(plan.height.max(vh))
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
        .absolute(vw - BAR_W - 4.0, 0.0)
        .w(BAR_W + 4.0)
        .h(vh)
        .live_opacity(page.scroll.bar_alpha.smooth(Scroller::bar_spring()))
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
        .h_full()
        .min_w(0.0)
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
        B::Chips => chips(page),
        B::Card(i) => items.get(i).map_or_else(el, |v| card(s, v, i, b.w, scale)),
        B::Row(i) => items.get(i).map_or_else(el, |v| {
            list_row(
                s,
                v,
                i,
                plan.row_thumb,
                scale,
                matches!(page.kind, Kind::History | Kind::Later),
            )
        }),
        B::Compact(i) => items.get(i).map_or_else(el, |v| compact(s, v, i, scale)),
        B::Status(st) => status(page, st),
        B::Banner => banner(s, page, b.w, scale),
        B::ChannelHead => channel_head(s, page, scale),
        B::Strip => strip(s, page, scale),
        B::LibHead => lib_head(page),
        B::Player => player(s, b),
        B::Info => watch_info(s, page, scale),
        B::Desc => description(page),
        B::CommentsHead => comments_head(page),
        B::Comment(i) => page
            .watch()
            .and_then(|w| w.comments.get(i))
            .map_or_else(el, |c| comment(s, c, i, scale)),
    }
}

// ----- small pieces -----------------------------------------------------------

fn px(w: f32, scale: f32) -> u32 {
    (w * scale).ceil() as u32
}

fn avatar(s: &App, url: Option<&str>, d: f32, scale: f32) -> El {
    let a = el().size(d, d).radius(d / 2.0).fill(t().raised);
    match url {
        Some(u) if !u.is_empty() => a.pixels(&s.images.get(u, px(d, scale))),
        _ => a,
    }
}

fn thumb(s: &App, v: &Video, w: f32, h: f32, scale: f32) -> El {
    let tok = t();
    let mut th = col()
        .w(w)
        .h(h)
        .radius(12.0)
        .fill(tok.raised)
        .pixels_fit(&s.images.get(&v.thumbnail, px(w, scale)), SurfaceFit::Fill)
        .padding(6.0)
        .justify_end()
        .items_end();
    let badge = |label: String, bg: Color| {
        el().px(4.0)
            .py(1.0)
            .radius(4.0)
            .fill(bg)
            .child(text(label).font(S12M).color(tok.text))
    };
    if v.is_live {
        th = th.child(badge("LIVE".into(), tok.accent));
    } else if let Some(d) = &v.duration {
        th = th.child(badge(d.clone(), tok.badge));
    }
    th
}

fn channel_press(v: &Video) -> impl Fn(&mut App) + 'static {
    let id = v.channel_id.clone().unwrap_or_default();
    move |s: &mut App| s.open_channel(id.clone())
}

fn card(s: &App, v: &Video, i: usize, w: f32, scale: f32) -> El {
    let tok = t();
    let th = w * 9.0 / 16.0;
    let open = v.clone();
    let has_channel = v.channel_id.is_some();
    let mut info = row().w_full().gap(12.0).pt(12.0);
    if v.channel_avatar.is_some() {
        let mut a = avatar(s, v.channel_avatar.as_deref(), 36.0, scale);
        if has_channel {
            a = a.on_press(channel_press(v));
        }
        info = info.child(a);
    }
    let mut name = text(v.channel.clone())
        .font(S14)
        .color(tok.text2)
        .truncate();
    if has_channel {
        name = name.on_press(channel_press(v));
    }
    info = info.child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .child(
                text(v.title.clone())
                    .font(S16M)
                    .color(tok.text)
                    .max_lines(2)
                    .mb(4.0),
            )
            .child(name)
            .child(text(v.meta_line()).font(S14).color(tok.text2).truncate()),
    );
    col()
        .key(format!("card-{i}-{}", v.id))
        .w(w)
        .radius(12.0)
        .hover(tok.wash)
        .on_press(move |s: &mut App| s.open_video(open.clone()))
        .child(thumb(s, v, w, th, scale))
        .child(info)
}

fn list_row(s: &App, v: &Video, i: usize, (tw, th): (f32, f32), scale: f32, lib: bool) -> El {
    let tok = t();
    let open = v.clone();
    let mut chan = row().gap(8.0).center_y().py(8.0);
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
        chan = chan.on_press(channel_press(v));
    }
    let mut info = col()
        .grow(1.0)
        .min_w(0.0)
        .child(text(v.title.clone()).font(S18).color(tok.text).max_lines(2))
        .child(text(v.meta_line()).font(S12).color(tok.text2).truncate())
        .child(chan);
    if let Some(sn) = &v.snippet {
        info = info.child(text(sn.clone()).font(S12).color(tok.text2).max_lines(2));
    }
    let _ = lib;
    row()
        .key(format!("row-{i}-{}", v.id))
        .w_full()
        .h(th)
        .gap(16.0)
        .radius(12.0)
        .hover(tok.wash)
        .on_press(move |s: &mut App| s.open_video(open.clone()))
        .child(thumb(s, v, tw, th, scale))
        .child(info)
}

fn compact(s: &App, v: &Video, i: usize, scale: f32) -> El {
    let tok = t();
    let open = v.clone();
    row()
        .key(format!("rel-{i}-{}", v.id))
        .w_full()
        .h(COMPACT_H)
        .gap(8.0)
        .radius(8.0)
        .hover(tok.wash)
        .on_press(move |s: &mut App| s.open_video(open.clone()))
        .child(thumb(s, v, 168.0, 94.0, scale))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(v.title.clone())
                        .font(S14M)
                        .color(tok.text)
                        .max_lines(2),
                )
                .child(
                    text(v.channel.clone())
                        .font(S12)
                        .color(tok.text2)
                        .truncate(),
                )
                .child(text(v.meta_line()).font(S12).color(tok.text2).truncate()),
        )
}

fn chips(page: &Page) -> El {
    let tok = t();
    let cur = match page.kind {
        Kind::Home { topic } => topic,
        _ => 0,
    };
    let mut r = row()
        .key("chips")
        .w_full()
        .h(CHIPS_H)
        .gap(12.0)
        .clip_soft(0.0);
    for (i, topic) in nt_core::TOPICS.iter().enumerate() {
        let on = i == cur;
        r = r.child(
            el().key(format!("chip-{i}"))
                .h(CHIPS_H)
                .px(12.0)
                .radius(8.0)
                .center()
                .role("button")
                .fill(if on { tok.chip_sel_bg } else { tok.raised })
                .hover(if on { Color::TRANSPARENT } else { tok.wash })
                .on_press(move |s: &mut App| s.set_topic(i))
                .child(text(topic.title).font(S14M).color(if on {
                    tok.chip_sel_fg
                } else {
                    tok.text
                })),
        );
    }
    r
}

fn retry_button() -> El {
    Button::new("Retry")
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::ExtraLarge)
        .on_press(|s: &mut App| s.retry())
        .build(kit())
}

fn status(page: &Page, st: St) -> El {
    let tok = t();
    let k = kit();
    let msg = |m: String| text(m).font(S14).color(tok.text2).wrap().max_lines(3);
    let center = col()
        .key(format!("status-{st:?}"))
        .w_full()
        .h_full()
        .center()
        .gap(16.0);
    match st {
        St::Loading | St::MoreLoading | St::CommentsLoading => {
            center.child(Spinner::new().size(28.0).color(tok.text2).build(k))
        }
        St::Error | St::MoreError | St::CommentsError => {
            let err = match st {
                St::Error => match (
                    &page.feed.status,
                    page.watch().and_then(|w| w.error.clone()),
                ) {
                    (_, Some(e)) => e,
                    (Load::Error(e), _) => e.clone(),
                    _ => String::new(),
                },
                St::MoreError => match &page.feed.more {
                    Load::Error(e) => e.clone(),
                    _ => String::new(),
                },
                _ => match page.watch().map(|w| &w.comments_state) {
                    Some(Load::Error(e)) => e.clone(),
                    _ => String::new(),
                },
            };
            center.child(msg(err)).child(retry_button())
        }
        St::Empty => {
            let (title, sub) = match page.kind {
                Kind::Subs { .. } => (
                    "No subscriptions yet",
                    "Subscribe to channels to see their newest videos here.",
                ),
                Kind::History => ("No watch history", "Videos you watch will show up here."),
                Kind::Later => (
                    "Nothing saved for later",
                    "Use Watch Later on a video to save it here.",
                ),
                Kind::Search { .. } => ("No results", "Try different keywords."),
                _ => ("Nothing here", ""),
            };
            center
                .child(text(title).font(S20).color(tok.text))
                .child(msg(sub.into()))
        }
    }
}

fn banner(s: &App, page: &Page, w: f32, scale: f32) -> El {
    let url = match &page.kind {
        Kind::Channel { info: Some(i), .. } => i.banner.clone().unwrap_or_default(),
        _ => String::new(),
    };
    el().key("banner")
        .w_full()
        .h(160.0)
        .radius(12.0)
        .fill(t().raised)
        .pixels_fit(&s.images.get(&url, px(w, scale)), SurfaceFit::Fill)
}

fn subscribe_button(ch: nt_core::ChannelRef) -> El {
    let subscribed = library().is_subscribed(&ch.id);
    let b = Button::new(if subscribed {
        "Subscribed"
    } else {
        "Subscribe"
    })
    .control_size(ControlSize::ExtraLarge)
    .on_press(move |s: &mut App| {
        library().toggle_subscription(&ch);
        s.top().touch();
    });
    if subscribed {
        b.variant(ButtonVariant::Neutral).build(kit())
    } else {
        b.variant(ButtonVariant::Contrast).build(accent_kit())
    }
}

fn accent_kit() -> &'static rune_kit::Kit {
    static K: std::sync::LazyLock<rune_kit::Kit> = std::sync::LazyLock::new(|| {
        let mut k = kit().clone();
        k.c.bg_contrast = t().accent;
        k.c.text_contrast = Color::WHITE;
        k
    });
    &K
}

fn channel_head(s: &App, page: &Page, scale: f32) -> El {
    let tok = t();
    let Kind::Channel {
        info: Some(info), ..
    } = &page.kind
    else {
        return el();
    };
    let line = [
        info.handle.clone(),
        info.subscribers.clone(),
        info.video_count.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" • ");
    row()
        .key("channel-head")
        .w_full()
        .h_full()
        .gap(24.0)
        .center_y()
        .child(avatar(s, info.avatar.as_deref(), 80.0, scale))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(
                    text(info.name.clone())
                        .font(S20B)
                        .color(tok.text)
                        .truncate(),
                )
                .child(text(line).font(S14).color(tok.text2).truncate())
                .child(
                    text(info.description.clone())
                        .font(S14)
                        .color(tok.text2)
                        .wrap()
                        .max_lines(2),
                ),
        )
        .child(subscribe_button(info.as_ref()))
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
        r = r.child(
            col()
                .key(format!("sub-{i}"))
                .w(88.0)
                .gap(8.0)
                .py(4.0)
                .items_center()
                .radius(12.0)
                .hover(tok.wash)
                .on_press(move |s: &mut App| s.open_channel(id.clone()))
                .child(avatar(s, c.avatar.as_deref(), 64.0, scale))
                .child(text(c.name.clone()).font(S12).color(tok.text).truncate()),
        );
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
            Button::new("Clear history")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::ExtraLarge)
                .on_press(|s: &mut App| {
                    library().clear_history();
                    s.refresh_library_page();
                })
                .build(kit()),
        );
    }
    r
}

// ----- watch page -------------------------------------------------------------

fn icon_button(
    key: &str,
    ic: &'static Icon,
    label: &'static str,
    f: impl Fn(&mut App) + 'static,
) -> El {
    el().key(key.to_string())
        .size(36.0, 36.0)
        .radius(18.0)
        .center()
        .role("button")
        .label(label)
        .hover(Color::WHITE.alpha(0.15))
        .on_press(f)
        .child(icon(ic, 22.0).color(Color::WHITE))
}

fn player(s: &App, b: &Block) -> El {
    let tok = t();
    let v = &s.video;
    let sh = &v.shown;
    let loading = v.error.is_none()
        && (sh.status == nt_core::Status::Loading
            || sh.buffering
            || sh.status == nt_core::Status::Idle);
    let failed = match (&v.error, &sh.status) {
        (Some(e), _) => Some(e.clone()),
        (_, nt_core::Status::Failed(e)) => Some(e.clone()),
        _ => None,
    };
    let mut p = el()
        .key("player")
        .w(b.w)
        .h(b.h)
        .radius(12.0)
        .fill(Color::BLACK)
        .on_hover_change(|s: &mut App, h: bool, _: &mut Tx| s.player_hover = h)
        .on_press(|s: &mut App| s.video.toggle_pause());
    if sh.has_picture {
        p = p.child(
            el().key("picture")
                .absolute(0.0, 0.0)
                .w(b.w)
                .h(b.h)
                .radius(12.0)
                .pixels_fit(video::pixels(), SurfaceFit::Fit),
        );
    }
    if let Some(e) = failed {
        p = p.child(
            col()
                .absolute(0.0, 0.0)
                .w(b.w)
                .h(b.h)
                .center()
                .px(24.0)
                .child(text(e).font(S14).color(tok.text).wrap().max_lines(4)),
        );
    } else if loading {
        p = p.child(
            col()
                .absolute(0.0, 0.0)
                .w(b.w)
                .h(b.h)
                .center()
                .child(Spinner::new().size(40.0).color(Color::WHITE).build(kit())),
        );
    }
    let show_controls = s.player_hover || sh.paused || sh.status == nt_core::Status::Ended;
    if show_controls && sh.status != nt_core::Status::Idle {
        p = p.child(controls(s, b));
    }
    p
}

fn controls(s: &App, b: &Block) -> El {
    let tok = t();
    let v = &s.video;
    let playing = !v.shown.paused && v.shown.status == nt_core::Status::Ready;
    let seek = el()
        .key("seek")
        .w(v.seek_w)
        .h(16.0)
        .center_y()
        .on_drag(|s: &mut App, d: &mut rune::Drag, _: &mut Tx| {
            if matches!(d.phase, Phase::Down | Phase::Move) {
                s.video.seek_frac(d.x / d.w.max(1.0));
            }
        })
        .child(
            el().w(v.seek_w)
                .h(4.0)
                .radius(2.0)
                .fill(Color::WHITE.alpha(0.25))
                .child(
                    el().absolute(0.0, 0.0)
                        .h(4.0)
                        .radius(2.0)
                        .fill(Color::WHITE.alpha(0.45))
                        .live_rect(&v.buffered),
                )
                .child(
                    el().absolute(0.0, 0.0)
                        .h(4.0)
                        .radius(2.0)
                        .fill(tok.accent)
                        .live_rect(&v.progress),
                ),
        );
    let vol_w = 72.0;
    let vol = if v.shown.muted {
        0.0
    } else {
        v.volume() as f32 / 100.0
    };
    let volume = el()
        .key("volume")
        .w(vol_w)
        .h(24.0)
        .center_y()
        .on_drag(|s: &mut App, d: &mut rune::Drag, _: &mut Tx| {
            if matches!(d.phase, Phase::Down | Phase::Move) {
                s.video
                    .set_volume(((d.x / d.w.max(1.0)).clamp(0.0, 1.0) * 100.0) as f64);
            }
        })
        .child(
            el().w(vol_w)
                .h(4.0)
                .radius(2.0)
                .fill(Color::WHITE.alpha(0.25))
                .child(
                    el().absolute(0.0, 0.0)
                        .w(vol_w * vol)
                        .h(4.0)
                        .radius(2.0)
                        .fill(Color::WHITE),
                ),
        );
    col()
        .key("controls")
        .absolute(0.0, b.h - 72.0)
        .w(b.w)
        .h(72.0)
        .px(12.0)
        .justify_end()
        .pb(6.0)
        .gradient(Color::TRANSPARENT, Color::BLACK.alpha(0.7))
        .corners([0.0, 0.0, 12.0, 12.0])
        .child(seek)
        .child(
            row()
                .w_full()
                .h(40.0)
                .gap(8.0)
                .center_y()
                .child(icon_button(
                    "play",
                    if playing { &PAUSE } else { &PLAY },
                    "Play or pause",
                    |s: &mut App| s.video.toggle_pause(),
                ))
                .child(icon_button(
                    "mute",
                    if v.shown.muted { &MUTED } else { &VOLUME },
                    "Mute",
                    |s: &mut App| s.video.toggle_mute(),
                ))
                .child(volume)
                .child(el().w(8.0))
                .child(
                    text(video::CLOCK_PLACEHOLDER)
                        .live_text(&v.clock)
                        .font(S12_TNUM)
                        .color(Color::WHITE),
                ),
        )
}

fn watch_info(s: &App, page: &Page, scale: f32) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let title = w
        .info
        .as_ref()
        .map(|i| i.title.clone())
        .unwrap_or_else(|| w.video.title.clone());
    let mut r = row().w_full().h(40.0).gap(12.0).center_y();
    let channel = w
        .info
        .as_ref()
        .map(|i| i.channel.clone())
        .unwrap_or_else(|| nt_core::ChannelRef {
            id: w.video.channel_id.clone().unwrap_or_default(),
            name: w.video.channel.clone(),
            avatar: w.video.channel_avatar.clone(),
            subscribers: None,
        });
    let id = channel.id.clone();
    r = r.child(avatar(s, channel.avatar.as_deref(), 40.0, scale).on_press({
        let id = id.clone();
        move |s: &mut App| s.open_channel(id.clone())
    }));
    r = r.child(
        col()
            .min_w(0.0)
            .shrink()
            .on_press(move |s: &mut App| s.open_channel(id.clone()))
            .child(
                text(channel.name.clone())
                    .font(S16M)
                    .color(tok.text)
                    .truncate(),
            )
            .child(
                text(channel.subscribers.clone().unwrap_or_default())
                    .font(S12)
                    .color(tok.text2)
                    .truncate(),
            ),
    );
    if !channel.id.is_empty() {
        r = r.child(el().w(8.0)).child(subscribe_button(channel));
    }
    r = r.child(el().grow(1.0));
    if let Some(likes) = w.info.as_ref().and_then(|i| i.likes.clone()) {
        r = r.child(
            row()
                .h(36.0)
                .px(16.0)
                .gap(8.0)
                .radius(18.0)
                .fill(tok.raised)
                .center_y()
                .child(icon(&LIKE, 18.0).color(tok.text))
                .child(text(likes).font(S14M).color(tok.text)),
        );
    }
    let video = w
        .info
        .as_ref()
        .map(|i| i.as_video())
        .unwrap_or_else(|| w.video.clone());
    let saved = library().is_saved(&video.id);
    r = r.child(
        Button::new(if saved { "Saved" } else { "Watch Later" })
            .icon(if saved { &SAVED } else { &LATER })
            .variant(ButtonVariant::Neutral)
            .control_size(ControlSize::ExtraLarge)
            .on_press(move |s: &mut App| {
                library().toggle_watch_later(&video);
                s.top().touch();
            })
            .build(kit()),
    );
    col()
        .key("watch-info")
        .w_full()
        .gap(12.0)
        .child(text(title).font(S20).color(tok.text).wrap())
        .child(r)
}

fn description(page: &Page) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let Some(info) = &w.info else { return el() };
    let head = [info.views.clone(), info.date.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ");
    let mut c = col()
        .key("desc")
        .w_full()
        .h_full()
        .padding(12.0)
        .radius(12.0)
        .fill(tok.raised)
        .gap(4.0)
        .child(text(head).font(S14B).color(tok.text));
    if !info.description.is_empty() {
        let mut d = text(info.description.clone())
            .font(S14)
            .color(tok.text)
            .wrap();
        if !w.desc_open {
            d = d.max_lines(DESC_LINES);
        }
        c = c.child(d);
        let full = rune::text::measure(
            &info.description,
            &[],
            S14,
            Some(page.plan.borrow().player[2] - 24.0),
        )
        .1;
        if full > 20.0 * DESC_LINES as f32 + 1.0 || w.desc_open {
            c = c.child(
                text(if w.desc_open {
                    "Show less"
                } else {
                    "Show more"
                })
                .font(S14B)
                .color(tok.text)
                .mt(4.0)
                .on_press(|s: &mut App| {
                    let p = s.top_mut();
                    if let Some(w) = p.watch_mut() {
                        w.desc_open = !w.desc_open;
                    }
                    p.touch();
                }),
            );
        }
    }
    c
}

fn comments_head(page: &Page) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let label = match &w.comments_total {
        Some(n) => format!("{n} Comments"),
        None => "Comments".to_string(),
    };
    row()
        .key("comments-head")
        .w_full()
        .h_full()
        .center_y()
        .child(text(label).font(S20B).color(tok.text))
}

fn comment(s: &App, c: &nt_core::Comment, i: usize, scale: f32) -> El {
    let tok = t();
    let mut meta = vec![c.author.clone()];
    if let Some(p) = &c.published {
        meta.push(p.clone());
    }
    let mut foot = row()
        .h(18.0)
        .gap(12.0)
        .center_y()
        .child(icon(&LIKE, 14.0).color(tok.text2));
    foot = foot.child(
        text(c.likes.clone().unwrap_or_default())
            .font(S12)
            .color(tok.text2),
    );
    if let Some(r) = c
        .reply_count
        .as_ref()
        .filter(|r| !r.is_empty() && r.as_str() != "0")
    {
        foot = foot.child(text(format!("{r} replies")).font(S12M).color(tok.text));
    }
    row()
        .key(format!("cm-{i}-{}", c.id))
        .w_full()
        .gap(12.0)
        .child(avatar(s, c.author_avatar.as_deref(), 40.0, scale))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(meta.join(" • "))
                        .font(S12M)
                        .color(tok.text2)
                        .truncate()
                        .mb(4.0),
                )
                .child(
                    text(c.text.clone())
                        .font(S14)
                        .color(tok.text)
                        .wrap()
                        .mb(6.0),
                )
                .child(foot),
        )
}
