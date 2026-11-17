//! Views. Every page is one `Scroller`; `view` builds only the items that intersect the band
//! [offset - viewport, offset + 2 viewports] (one viewport of overscan each side, plus the
//! half viewport the scroller lets the offset drift before it asks for a rebuild).

use crate::app::{App, Body, Comments, Feed, ListRef, Load, Message, More, Root, Watch};
use crate::scroller::{Item, Lane, ScrollModel, Scroller, frame_hook};
use crate::theme::{self as t, MEDIUM, PAD, RAIL, TOP_BAR};
use crate::video::VideoProgram;
use crate::widgets::{clamp, search_box, seek_bar, spinner, thumb};
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::widget::{
    Space, button, column, container, hover, mouse_area, row, shader, slider, stack, text,
    text_input,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Pixels, Theme};
use nt_core::{Comment, Status, TOPICS, Video, format, library};

type El<'a> = Element<'a, Message>;

// Item kinds (part of the key, so each kind's widget state survives band moves).
const K_HEAD: u64 = 1;
const K_STATUS: u64 = 2;
const K_ROW: u64 = 3;
const K_TRAIL: u64 = 4;
const K_PLAYER: u64 = 5;
const K_INFO: u64 = 6;
const K_DESC: u64 = 7;
const K_CHEAD: u64 = 8;
const K_COMMENT: u64 = 9;
const K_CTRAIL: u64 = 10;
const K_RHEAD: u64 = 11;
const K_REL: u64 = 12;
const K_BANNER: u64 = 13;
const K_STRIP: u64 = 14;

fn key(lane: u64, kind: u64, idx: usize) -> u64 {
    (lane << 56) | (kind << 40) | idx as u64
}

/// Collects one lane's items, building elements only for those inside the band.
struct LaneB<'a> {
    model: &'a ScrollModel,
    lane: u64,
    x: f32,
    width: f32,
    lo: f32,
    hi: f32,
    y: f32,
    top: Option<f32>,
    items: Vec<Item<'a, Message>>,
}

impl<'a> LaneB<'a> {
    fn new(model: &'a ScrollModel, lane: u64, x: f32, width: f32, offset: f32, vh: f32) -> Self {
        LaneB {
            model,
            lane,
            x,
            width,
            lo: offset - vh,
            hi: offset + 2.0 * vh,
            y: PAD,
            top: None,
            items: Vec::new(),
        }
    }

    fn visible(&self, h: f32) -> bool {
        self.y + h >= self.lo && self.y <= self.hi
    }

    fn item(&mut self, kind: u64, idx: usize, estimate: f32, build: impl FnOnce() -> El<'a>) {
        let key = key(self.lane, kind, idx);
        let h = self.model.height(key).unwrap_or(estimate);
        if self.visible(h) {
            self.top.get_or_insert(self.y);
            self.items.push(Item {
                key,
                assumed: h,
                element: build(),
            });
        }
        self.y += h;
    }

    /// `n` rows of a fixed height: O(visible) whatever `n` is.
    fn rows(&mut self, kind: u64, n: usize, row_h: f32, mut build: impl FnMut(usize) -> El<'a>) {
        if n == 0 {
            return;
        }
        let first = ((self.lo - self.y) / row_h).floor().max(0.0) as usize;
        let last = (((self.hi - self.y) / row_h).ceil().max(0.0) as usize).min(n);
        for i in first..last {
            let y = self.y + i as f32 * row_h;
            self.top.get_or_insert(y);
            let el = container(build(i)).height(row_h).width(Fill).into();
            self.items.push(Item {
                key: key(self.lane, kind, i),
                assumed: row_h,
                element: el,
            });
        }
        self.y += n as f32 * row_h;
    }

    fn finish(self) -> Lane<'a, Message> {
        Lane {
            x: self.x,
            width: self.width,
            top: self.top.unwrap_or(0.0),
            assumed_total: self.y,
            items: self.items,
        }
    }
}

fn txt<'a>(
    s: impl text::IntoFragment<'a>,
    size: f32,
    lh: f32,
) -> text::Text<'a, Theme, iced::Renderer> {
    text(s)
        .size(size)
        .line_height(LineHeight::Absolute(Pixels(lh)))
        .shaping(Shaping::Advanced)
}

fn badge_text(v: &Video) -> Option<String> {
    if v.is_live {
        Some("LIVE".to_string())
    } else {
        v.duration.clone()
    }
}

fn accent_pill(_: &Theme, status: button::Status) -> button::Style {
    let mut s = t::pill(&Theme::Dark, status);
    s.background = Some(iced::Background::Color(
        if matches!(status, button::Status::Hovered) {
            Color::from_rgb8(0xff, 0x55, 0x4b)
        } else {
            t::ACCENT
        },
    ));
    s.text_color = Color::WHITE;
    s
}

// ---------------------------------------------------------------------------------------------

pub fn view(app: &App) -> El<'_> {
    app.images.begin_view();
    let body = container(page(app)).width(Fill).height(Fill);
    let main = column![top_bar(app), row![rail(app), body].height(Fill)];
    let drop: El<'_> = if app.sugg_open && !app.suggestions.is_empty() {
        dropdown(app)
    } else {
        Space::new().into()
    };
    frame_hook(
        container(stack![main, drop])
            .style(t::surface)
            .width(Fill)
            .height(Fill),
    )
}

impl App {
    pub fn view(&self) -> El<'_> {
        view(self)
    }
}

fn side(app: &App) -> (f32, f32) {
    if app.win.width < 900.0 {
        (150.0, 24.0)
    } else {
        (190.0, 190.0)
    }
}

fn top_bar(app: &App) -> El<'_> {
    let back = button(container(txt("←", 20.0, 24.0)).center(Fill))
        .width(40)
        .height(40)
        .padding(0)
        .style(t::icon_button)
        .on_press_maybe((app.pages.len() > 1).then_some(Message::Back));
    let (left, right) = side(app);
    let input = text_input("Search", &app.query)
        .id(crate::app::SEARCH_ID)
        .on_input(Message::SearchInput)
        .on_submit(Message::SearchSubmit)
        .size(16)
        .padding(Padding {
            top: 9.0,
            bottom: 9.0,
            left: 16.0,
            right: 16.0,
        })
        .style(t::search_input)
        .width(Fill);
    let field = search_box(
        input,
        app.search_focused,
        Message::SearchFocus,
        Message::SuggestNav,
        app.sugg_open.then_some(Message::SuggestClose),
    )
    .blur(app.blur);
    let go = button(container(txt("🔍", 16.0, 20.0)).center(Fill))
        .width(64)
        .height(40)
        .padding(0)
        .style(t::search_button)
        .on_press(Message::SearchSubmit);
    let search = row![field, go].height(40).align_y(Alignment::Center);
    row![
        container(
            row![back, txt("NativeTube", 18.0, 24.0).font(MEDIUM)]
                .spacing(12)
                .align_y(Alignment::Center)
        )
        .width(left),
        Space::new().width(FillPortion(1)),
        container(search).width(FillPortion(6)).max_width(640),
        Space::new().width(FillPortion(1)),
        Space::new().width(right),
    ]
    .height(TOP_BAR)
    .padding(Padding {
        left: 16.0,
        right: 16.0,
        top: 0.0,
        bottom: 0.0,
    })
    .align_y(Alignment::Center)
    .into()
}

fn dropdown(app: &App) -> El<'_> {
    let (left, right) = side(app);
    let items = app.suggestions.iter().enumerate().map(|(i, s)| {
        button(
            row![
                txt("🔍", 14.0, 20.0).color(t::TEXT_2),
                txt(s.as_str(), 16.0, 20.0)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 16.0,
            right: 16.0,
        })
        .style(t::suggestion(app.sugg_sel == Some(i)))
        .on_press(Message::SuggestPick(i))
        .into()
    });
    let list = container(column(items))
        .style(t::dropdown)
        .padding(Padding {
            top: 8.0,
            bottom: 8.0,
            left: 0.0,
            right: 0.0,
        })
        .width(Fill);
    column![
        Space::new().height(TOP_BAR - 4.0),
        row![
            Space::new().width(left),
            Space::new().width(FillPortion(1)),
            container(row![list, Space::new().width(64)])
                .width(FillPortion(6))
                .max_width(640),
            Space::new().width(FillPortion(1)),
            Space::new().width(right),
        ]
        .padding(Padding {
            left: 16.0,
            right: 16.0,
            top: 0.0,
            bottom: 0.0
        })
    ]
    .into()
}

fn rail(app: &App) -> El<'_> {
    let root = if app.pages.len() == 1 {
        app.root()
    } else {
        None
    };
    let entries = [
        (Root::Home, "⌂", "Home"),
        (Root::Subscriptions, "▤", "Subscriptions"),
        (Root::History, "⟲", "History"),
        (Root::WatchLater, "◷", "Watch Later"),
    ];
    let col = column(entries.into_iter().map(|(r, icon, label)| {
        button(
            column![
                txt(icon, 20.0, 24.0),
                txt(label, 12.0, 14.0)
                    .wrapping(Wrapping::None)
                    .align_x(Alignment::Center)
                    .width(Fill)
            ]
            .align_x(Alignment::Center)
            .spacing(4),
        )
        .width(Fill)
        .padding(Padding {
            top: 12.0,
            bottom: 10.0,
            left: 0.0,
            right: 0.0,
        })
        .style(t::rail_entry(root == Some(r)))
        .on_press(Message::Nav(r))
        .into()
    }))
    .spacing(4);
    container(col)
        .width(RAIL)
        .height(Fill)
        .padding(Padding {
            top: 4.0,
            bottom: 0.0,
            left: 2.0,
            right: 2.0,
        })
        .into()
}

// ---------------------------------------------------------------------------------------------

struct Geo {
    lane_w: f32,
    vh: f32,
    offset: f32,
}

fn page(app: &App) -> El<'_> {
    let p = app.top();
    let body_w = (app.win.width - RAIL).max(200.0);
    let vh = (app.win.height - TOP_BAR).max(100.0);
    let g = Geo {
        lane_w: body_w - 2.0 * PAD,
        vh,
        offset: p.scroll.mark_built(),
    };
    let model = &p.scroll;
    let mut on_key: Option<crate::scroller::KeyHandler<'_, Message>> = None;
    let lanes = match &p.body {
        Body::Home(h) => {
            let mut l = LaneB::new(model, 0, PAD, g.lane_w, g.offset, g.vh);
            l.item(K_HEAD, 0, 56.0, || chips(h.topic));
            feed_grid(
                app,
                &mut l,
                &h.load,
                &h.feed,
                &g,
                "No videos for this topic.",
            );
            vec![l.finish()]
        }
        Body::Search(s) => {
            let mut l = LaneB::new(model, 0, PAD, g.lane_w, g.offset, g.vh);
            match &s.load {
                Load::Ready if s.feed.items.is_empty() => l.item(K_STATUS, 0, 200.0, || {
                    empty(format!("No results for \u{201c}{}\u{201d}.", s.query))
                }),
                Load::Ready => {
                    list_rows(app, &mut l, &s.feed.items, &g);
                    trailer(&mut l, &s.feed);
                }
                other => status_item(&mut l, other),
            }
            vec![l.finish()]
        }
        Body::Channel(c) => {
            let mut l = LaneB::new(model, 0, PAD, g.lane_w, g.offset, g.vh);
            if let Some(banner) = c.info.as_ref().and_then(|i| i.banner.as_ref()) {
                let w = g.lane_w;
                l.item(K_BANNER, 0, 176.0, || {
                    container(
                        thumb(app.images.get(banner, w * app.scale), w, 160.0, t::R_THUMB).cover(),
                    )
                    .padding(Padding {
                        bottom: 16.0,
                        ..Padding::ZERO
                    })
                    .into()
                });
            }
            l.item(K_HEAD, 0, 150.0, || channel_header(app, c));
            feed_grid(
                app,
                &mut l,
                &c.load,
                &c.feed,
                &g,
                "This channel has no videos.",
            );
            vec![l.finish()]
        }
        Body::Subs(s) => {
            let mut l = LaneB::new(model, 0, PAD, g.lane_w, g.offset, g.vh);
            if s.channels.is_empty() {
                l.item(K_STATUS, 0, 200.0, || empty("You have no subscriptions yet. Subscribe to channels from a video or channel page.".to_string()));
            } else {
                l.item(K_STRIP, 0, 120.0, || strip(app, s));
                l.item(K_HEAD, 0, 52.0, || section("Latest"));
                feed_grid(app, &mut l, &s.load, &s.feed, &g, "No recent uploads.");
            }
            vec![l.finish()]
        }
        Body::History(items) | Body::Later(items) => {
            let history = matches!(p.body, Body::History(_));
            let mut l = LaneB::new(model, 0, PAD, g.lane_w, g.offset, g.vh);
            l.item(K_HEAD, 0, 64.0, || {
                let title =
                    txt(if history { "History" } else { "Watch Later" }, 20.0, 28.0).font(MEDIUM);
                let mut r = row![title, Space::new().width(Fill)].align_y(Alignment::Center);
                if history && !items.is_empty() {
                    r = r.push(
                        button(txt("Clear history", 14.0, 20.0))
                            .padding([8, 16])
                            .style(t::pill)
                            .on_press(Message::ClearHistory),
                    );
                }
                container(r)
                    .height(64)
                    .padding(Padding {
                        bottom: 24.0,
                        ..Padding::ZERO
                    })
                    .into()
            });
            if items.is_empty() {
                let msg = if history {
                    "Videos you watch will show up here."
                } else {
                    "Save videos to watch later and they will show up here."
                };
                l.item(K_STATUS, 0, 200.0, || empty(msg.to_string()));
            } else {
                list_rows(app, &mut l, items, &g);
            }
            vec![l.finish()]
        }
        Body::Watch(w) => {
            on_key = Some(Box::new(App::watch_key_message));
            watch(app, w, model, &g)
        }
    };
    Scroller::new(model, lanes, Message::ScrollMoved)
        .on_key(on_key)
        .keys_enabled(!app.search_focused)
        .scale(app.scale)
        .into()
}

fn section(label: &str) -> El<'_> {
    container(txt(label, 20.0, 28.0).font(MEDIUM))
        .height(52)
        .padding(Padding {
            top: 0.0,
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}

fn empty<'a>(msg: String) -> El<'a> {
    container(txt(msg, 16.0, 22.0).color(t::TEXT_2))
        .width(Fill)
        .height(200)
        .center(Fill)
        .into()
}

fn error_block<'a>(msg: &str, retry: Message) -> El<'a> {
    container(
        column![
            txt("Something went wrong", 18.0, 24.0).font(MEDIUM),
            txt(msg.to_string(), 14.0, 20.0)
                .color(t::TEXT_2)
                .align_x(Alignment::Center),
            button(txt("Retry", 14.0, 20.0))
                .padding([8, 20])
                .style(t::pill)
                .on_press(retry)
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .max_width(560),
    )
    .width(Fill)
    .center_x(Fill)
    .padding(24)
    .into()
}

fn status_item(l: &mut LaneB<'_>, load: &Load) {
    match load {
        Load::Loading => l.item(K_STATUS, 0, 240.0, || {
            container(spinner(40.0))
                .width(Fill)
                .height(240)
                .center(Fill)
                .into()
        }),
        Load::Failed(e) => {
            let e = e.clone();
            l.item(K_STATUS, 1, 240.0, move || error_block(&e, Message::Retry))
        }
        Load::Ready => {}
    }
}

fn trailer(l: &mut LaneB<'_>, feed: &Feed) {
    match &feed.more {
        Some(More::Loading) => l.item(K_TRAIL, 0, 72.0, || {
            container(spinner(32.0))
                .width(Fill)
                .height(72)
                .center(Fill)
                .into()
        }),
        Some(More::Failed(e)) => {
            let e = e.clone();
            l.item(K_TRAIL, 1, 160.0, move || {
                error_block(&e, Message::RetryMore)
            })
        }
        _ => {}
    }
}

fn chips(selected: usize) -> El<'static> {
    let r = row(TOPICS.iter().enumerate().map(|(i, topic)| {
        button(txt(topic.title, 14.0, 20.0).font(MEDIUM))
            .padding([6, 12])
            .height(32)
            .style(t::chip(i == selected))
            .on_press(Message::Topic(i))
            .into()
    }))
    .spacing(12);
    container(r).height(56).clip(true).into()
}

/// Columns that fit with a card width in [300, 420] and a 16 px gap.
fn grid_geom(lane_w: f32) -> (usize, f32) {
    let cols = (((lane_w + 16.0) / 316.0).floor() as usize).max(1);
    let w = ((lane_w - (cols - 1) as f32 * 16.0) / cols as f32).min(420.0);
    (cols, w.floor())
}

fn card_height(w: f32) -> f32 {
    (w * 9.0 / 16.0).round() + 12.0 + 88.0 + 8.0
}

fn feed_grid<'a>(
    app: &'a App,
    l: &mut LaneB<'a>,
    load: &'a Load,
    feed: &'a Feed,
    g: &Geo,
    none: &'static str,
) {
    match load {
        Load::Ready if feed.items.is_empty() => {
            l.item(K_STATUS, 2, 200.0, || empty(none.to_string()))
        }
        Load::Ready => {
            let (cols, w) = grid_geom(g.lane_w);
            let row_h = card_height(w) + 24.0;
            let items = &feed.items;
            let n = items.len().div_ceil(cols);
            l.rows(K_ROW, n, row_h, |r| {
                let start = r * cols;
                let end = (start + cols).min(items.len());
                row((start..end).map(|i| card(app, &items[i], i, ListRef::Main, w)))
                    .spacing(16)
                    .into()
            });
            trailer(l, feed);
        }
        other => status_item(l, other),
    }
}

fn avatar<'a>(app: &'a App, v: &'a Video, i: usize, list: ListRef, size: f32) -> Option<El<'a>> {
    let url = v.channel_avatar.as_ref()?;
    let img = thumb(
        app.images.get(url, size * app.scale),
        size,
        size,
        size / 2.0,
    );
    Some(
        button(img)
            .padding(0)
            .style(t::bare)
            .on_press_maybe(
                v.channel_id
                    .as_ref()
                    .map(|_| Message::OpenChannelOf(list, i)),
            )
            .into(),
    )
}

fn channel_name<'a>(v: &'a Video, i: usize, list: ListRef, size: f32, lh: f32) -> El<'a> {
    let name = clamp(v.channel.as_str(), size, lh, 1);
    match &v.channel_id {
        Some(_) if !v.channel.is_empty() => button(name)
            .padding(0)
            .style(t::link)
            .on_press(Message::OpenChannelOf(list, i))
            .into(),
        _ => container(name.color(t::TEXT_2)).into(),
    }
}

fn card<'a>(app: &'a App, v: &'a Video, i: usize, list: ListRef, w: f32) -> El<'a> {
    let th = (w * 9.0 / 16.0).round();
    let img = thumb(
        app.images.get(&v.thumbnail, w * app.scale),
        w,
        th,
        t::R_THUMB,
    )
    .cover()
    .badge(badge_text(v), v.is_live);
    let text_col = column![
        clamp(v.title.as_str(), 16.0, 22.0, 2)
            .font(MEDIUM)
            .reserve(),
        Space::new().height(4),
        container(channel_name(v, i, list, 14.0, 20.0)).height(20),
        clamp(v.meta_line(), 14.0, 20.0, 1).color(t::TEXT_2),
    ]
    .width(Fill);
    let mut info = row![].spacing(12);
    if let Some(a) = avatar(app, v, i, list, 36.0) {
        info = info.push(a);
    }
    info = info.push(text_col);
    button(column![img, info].spacing(12))
        .width(w)
        .height(card_height(w))
        .padding(Padding {
            bottom: 8.0,
            ..Padding::ZERO
        })
        .style(t::card)
        .on_press(Message::Open(list, i))
        .into()
}

fn list_geom(lane_w: f32) -> (f32, f32) {
    let tw = if lane_w >= 760.0 {
        360.0
    } else {
        (lane_w * 0.45).clamp(160.0, 360.0).floor()
    };
    (tw, (tw * 202.0 / 360.0).round())
}

fn list_rows<'a>(app: &'a App, l: &mut LaneB<'a>, items: &'a [Video], g: &Geo) {
    let (tw, th) = list_geom(g.lane_w);
    l.rows(K_ROW, items.len(), th + 16.0, |i| {
        list_row(app, &items[i], i, ListRef::Main, tw, th)
    });
}

fn list_row<'a>(app: &'a App, v: &'a Video, i: usize, list: ListRef, tw: f32, th: f32) -> El<'a> {
    let img = thumb(
        app.images.get(&v.thumbnail, tw * app.scale),
        tw,
        th,
        t::R_THUMB,
    )
    .cover()
    .badge(badge_text(v), v.is_live);
    let mut chan = row![].spacing(8).align_y(Alignment::Center);
    if let Some(a) = avatar(app, v, i, list, 24.0) {
        chan = chan.push(a);
    }
    chan = chan.push(channel_name(v, i, list, 12.0, 16.0));
    let mut col = column![
        clamp(v.title.as_str(), 18.0, 24.0, 2).font(MEDIUM),
        clamp(v.meta_line(), 12.0, 16.0, 1).color(t::TEXT_2),
        chan,
    ]
    .spacing(8)
    .width(Fill);
    if let Some(s) = &v.snippet {
        col = col.push(clamp(s.as_str(), 12.0, 16.0, 2).color(t::TEXT_2));
    }
    button(row![img, col].spacing(16))
        .width(Fill)
        .height(th)
        .padding(0)
        .style(t::card)
        .on_press(Message::Open(list, i))
        .into()
}

fn compact_row<'a>(app: &'a App, v: &'a Video, i: usize) -> El<'a> {
    let img = thumb(
        app.images.get(&v.thumbnail, 168.0 * app.scale),
        168.0,
        94.0,
        8.0,
    )
    .cover()
    .badge(badge_text(v), v.is_live);
    let col = column![
        clamp(v.title.as_str(), 14.0, 20.0, 2).font(MEDIUM),
        clamp(v.channel.as_str(), 12.0, 16.0, 1).color(t::TEXT_2),
        clamp(v.meta_line(), 12.0, 16.0, 1).color(t::TEXT_2),
    ]
    .spacing(4)
    .width(Fill);
    button(row![img, col].spacing(8))
        .width(Fill)
        .height(94)
        .padding(0)
        .style(t::card)
        .on_press(Message::Open(ListRef::Related, i))
        .into()
}

fn subscribe_button<'a>(subscribed: bool) -> El<'a> {
    let b = button(
        txt(
            if subscribed {
                "Subscribed"
            } else {
                "Subscribe"
            },
            14.0,
            20.0,
        )
        .font(MEDIUM),
    )
    .padding([8, 16])
    .on_press(Message::ToggleSubscribe);
    if subscribed {
        b.style(t::pill).into()
    } else {
        b.style(accent_pill).into()
    }
}

fn channel_header<'a>(app: &'a App, c: &'a crate::app::Channel) -> El<'a> {
    let (name, avatar_url, line, desc) = match &c.info {
        Some(i) => {
            let parts: Vec<&str> = [
                i.handle.as_deref(),
                i.subscribers.as_deref(),
                i.video_count.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect();
            (
                i.name.as_str(),
                i.avatar.as_ref(),
                parts.join(" • "),
                i.description.as_str(),
            )
        }
        None => (
            c.stub.name.as_str(),
            c.stub.avatar.as_ref(),
            String::new(),
            "",
        ),
    };
    let av = thumb(
        avatar_url.and_then(|u| app.images.get(u, 80.0 * app.scale)),
        80.0,
        80.0,
        40.0,
    );
    let mut col = column![txt(name, 20.0, 28.0).font(MEDIUM)]
        .spacing(6)
        .width(Fill);
    if !line.is_empty() {
        col = col.push(txt(line, 14.0, 20.0).color(t::TEXT_2));
    }
    if !desc.is_empty() {
        col = col.push(clamp(desc, 14.0, 20.0, 2).color(t::TEXT_2));
    }
    col = col
        .push(Space::new().height(4))
        .push(subscribe_button(app.is_subscribed(&c.id)));
    container(row![av, col].spacing(16))
        .padding(Padding {
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}

fn strip<'a>(app: &'a App, s: &'a crate::app::Subs) -> El<'a> {
    let items = s.channels.iter().map(|c| {
        let av = thumb(
            c.avatar
                .as_ref()
                .and_then(|u| app.images.get(u, 64.0 * app.scale)),
            64.0,
            64.0,
            32.0,
        );
        button(
            column![av, clamp(c.name.as_str(), 12.0, 16.0, 1)]
                .spacing(6)
                .align_x(Alignment::Center),
        )
        .width(88)
        .padding(4)
        .style(t::card)
        .on_press(Message::OpenChannel(c.clone()))
        .into()
    });
    container(row(items).spacing(8).wrap().vertical_spacing(8))
        .padding(Padding {
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}

// ---------------------------------------------------------------------------------------------
// Watch page

fn watch<'a>(
    app: &'a App,
    w: &'a Watch,
    model: &'a ScrollModel,
    g: &Geo,
) -> Vec<Lane<'a, Message>> {
    let wide = app.win.width >= 1000.0;
    let primary_w = if wide {
        g.lane_w - 24.0 - 400.0
    } else {
        g.lane_w
    }
    .floor();
    let player_h = (primary_w * 9.0 / 16.0).min(app.win.height * 0.7).floor();
    let mut l = LaneB::new(model, 0, PAD, primary_w, g.offset, g.vh);
    l.item(K_PLAYER, 0, player_h, || player(app, primary_w, player_h));
    l.item(K_INFO, 0, 140.0, || watch_info(app, w));
    match &w.load {
        Load::Ready => {
            if let Some(info) = &w.info {
                l.item(K_DESC, 0, 140.0, || description(info, w.desc_open));
            }
            w.comments_y.set(l.y);
            l.item(K_CHEAD, 0, 68.0, || {
                let label = match &w.comments_total {
                    Some(n) => format!("{n} Comments"),
                    None => "Comments".to_string(),
                };
                container(txt(label, 20.0, 28.0).font(MEDIUM))
                    .padding(Padding {
                        top: 24.0,
                        bottom: 16.0,
                        ..Padding::ZERO
                    })
                    .into()
            });
            let text_w = (primary_w - 56.0).max(100.0);
            for (i, c) in w.comments.iter().enumerate() {
                let est_lines = ((c.text.chars().count() as f32 * 7.2) / text_w)
                    .ceil()
                    .max(1.0)
                    + c.text.matches('\n').count() as f32;
                l.item(
                    K_COMMENT,
                    i,
                    20.0 + 4.0 + est_lines * 20.0 + 4.0 + 16.0 + 16.0,
                    || comment(app, c),
                );
            }
            match &w.comments_state {
                Comments::Loading => l.item(K_CTRAIL, 0, 72.0, || {
                    container(spinner(32.0))
                        .width(Fill)
                        .height(72)
                        .center(Fill)
                        .into()
                }),
                Comments::Failed(e) => {
                    let e = e.clone();
                    l.item(K_CTRAIL, 1, 160.0, move || error_block(&e, Message::Retry))
                }
                Comments::NotStarted if w.info.as_ref().is_some_and(|i| i.comments.is_none()) => l
                    .item(K_CTRAIL, 2, 60.0, || {
                        container(txt("Comments are turned off.", 14.0, 20.0).color(t::TEXT_2))
                            .height(60)
                            .into()
                    }),
                _ => {}
            }
        }
        other => {
            w.comments_y.set(f32::INFINITY);
            status_item(&mut l, other);
        }
    }
    if wide {
        let mut r = LaneB::new(model, 1, PAD + primary_w + 24.0, 400.0, g.offset, g.vh);
        r.rows(K_REL, w.related.len(), 102.0, |i| {
            compact_row(app, &w.related[i], i)
        });
        vec![l.finish(), r.finish()]
    } else {
        if !w.related.is_empty() {
            l.item(K_RHEAD, 0, 76.0, || {
                container(txt("Related", 20.0, 28.0).font(MEDIUM))
                    .padding(Padding {
                        top: 32.0,
                        bottom: 16.0,
                        ..Padding::ZERO
                    })
                    .into()
            });
            l.rows(K_REL, w.related.len(), 102.0, |i| {
                compact_row(app, &w.related[i], i)
            });
        }
        vec![l.finish()]
    }
}

fn player(app: &App, pw: f32, ph: f32) -> El<'_> {
    let st = &app.play;
    let mut base_layers: Vec<El<'_>> = vec![
        container(Space::new())
            .width(pw)
            .height(ph)
            .style(t::black)
            .into(),
    ];
    if let Some(f) = &app.frame {
        let aspect = st
            .aspect()
            .unwrap_or(f.width as f32 / f.height.max(1) as f32);
        let (mut fw, mut fh) = (pw, pw / aspect);
        if fh > ph {
            fh = ph;
            fw = ph * aspect;
        }
        let (fw, fh) = (fw.floor(), fh.floor());
        if let Some(p) = &app.player {
            p.set_target_size((fw * app.scale) as u32, (fh * app.scale) as u32);
        }
        base_layers.push(
            container(
                shader(VideoProgram {
                    frame: Some(f.clone()),
                })
                .width(fw)
                .height(fh),
            )
            .width(pw)
            .height(ph)
            .center(Length::Fixed(pw))
            .into(),
        );
    } else if let Some(p) = &app.player {
        let (fw, fh) = (pw, ph);
        p.set_target_size((fw * app.scale) as u32, (fh * app.scale) as u32);
    }
    let error = match (&st.status, &app.player_error) {
        (_, Some(e)) => Some(e.clone()),
        (Status::Failed(e), _) => Some(e.clone()),
        _ => None,
    };
    if let Some(e) = error {
        base_layers.push(
            container(
                column![
                    txt("Playback failed", 18.0, 24.0).font(MEDIUM),
                    txt(e, 14.0, 20.0)
                        .color(t::TEXT_2)
                        .align_x(Alignment::Center),
                    button(txt("Retry", 14.0, 20.0))
                        .padding([8, 20])
                        .style(t::pill)
                        .on_press(Message::RetryPlayback)
                ]
                .spacing(12)
                .align_x(Alignment::Center)
                .max_width(pw - 48.0),
            )
            .width(pw)
            .height(ph)
            .center(Length::Fixed(pw))
            .into(),
        );
    } else if st.status == Status::Loading
        || st.buffering
        || (app.frame.is_none() && st.status != Status::Idle)
    {
        base_layers.push(
            container(spinner(48.0))
                .width(pw)
                .height(ph)
                .center(Length::Fixed(pw))
                .into(),
        );
    }
    let base = mouse_area(stack(base_layers)).on_press(Message::PlayPause);

    let played = if st.duration > 0.0 {
        (st.position / st.duration) as f32
    } else {
        0.0
    };
    let buffered = if st.duration > 0.0 {
        ((st.position + st.buffered_ahead) / st.duration) as f32
    } else {
        0.0
    };
    let paused = st.paused || st.status == Status::Ended;
    let play_btn = button(container(txt(if paused { "▶" } else { "❚❚" }, 16.0, 20.0)).center(Fill))
        .width(36)
        .height(36)
        .padding(0)
        .style(t::control_button)
        .on_press(Message::PlayPause);
    let mute_btn = button(
        container(txt(
            if st.muted || st.volume <= 0.0 {
                "🔇"
            } else {
                "🔊"
            },
            16.0,
            20.0,
        ))
        .center(Fill),
    )
    .width(36)
    .height(36)
    .padding(0)
    .style(t::control_button)
    .on_press(Message::ToggleMute);
    let vol = slider(
        0.0..=100.0,
        if st.muted { 0.0 } else { st.volume as f32 },
        Message::SetVolume,
    )
    .step(1.0_f32)
    .width(90);
    let time = txt(
        format!(
            "{} / {}",
            format::clock(st.position),
            format::clock(st.duration)
        ),
        14.0,
        20.0,
    );
    let bar = container(
        column![
            seek_bar(played, buffered, Message::SeekFrac),
            row![play_btn, mute_btn, vol, Space::new().width(8), time]
                .spacing(4)
                .align_y(Alignment::Center)
        ]
        .spacing(2),
    )
    .width(Fill)
    .padding(Padding {
        top: 24.0,
        bottom: 6.0,
        left: 12.0,
        right: 12.0,
    })
    .style(t::controls_scrim);
    let controls = column![
        mouse_area(Space::new().width(Fill).height(Fill)).on_press(Message::PlayPause),
        bar
    ]
    .width(pw)
    .height(ph);
    let layered: El<'_> = if paused || st.status == Status::Idle {
        stack![base, controls].into()
    } else {
        hover(base, controls)
    };
    container(layered)
        .width(pw)
        .height(ph + 12.0)
        .align_top(ph + 12.0)
        .into()
}

fn watch_info<'a>(app: &'a App, w: &'a Watch) -> El<'a> {
    let title = w
        .info
        .as_ref()
        .map(|i| i.title.as_str())
        .unwrap_or(w.video.title.as_str());
    let (ch_name, ch_avatar, subs, ch_id) = match &w.info {
        Some(i) => (
            i.channel.name.as_str(),
            i.channel.avatar.as_ref(),
            i.channel.subscribers.as_deref(),
            i.channel.id.as_str(),
        ),
        None => (
            w.video.channel.as_str(),
            w.video.channel_avatar.as_ref(),
            None,
            w.video.channel_id.as_deref().unwrap_or(""),
        ),
    };
    let av = button(thumb(
        ch_avatar.and_then(|u| app.images.get(u, 40.0 * app.scale)),
        40.0,
        40.0,
        20.0,
    ))
    .padding(0)
    .style(t::bare)
    .on_press(Message::OpenWatchChannel);
    let mut names = column![
        button(txt(ch_name, 16.0, 22.0).font(MEDIUM))
            .padding(0)
            .style(t::bare)
            .on_press(Message::OpenWatchChannel)
    ];
    if let Some(s) = subs {
        names = names.push(txt(s, 12.0, 16.0).color(t::TEXT_2));
    }
    let mut r = row![av, names].spacing(12).align_y(Alignment::Center);
    if w.info.is_some() && !ch_id.is_empty() {
        r = r
            .push(Space::new().width(12))
            .push(subscribe_button(app.is_subscribed(ch_id)));
    }
    r = r.push(Space::new().width(Fill));
    if let Some(likes) = w.info.as_ref().and_then(|i| i.likes.as_deref()) {
        r = r.push(
            container(txt(format!("👍 {likes}"), 14.0, 20.0).font(MEDIUM))
                .padding([8, 16])
                .style(|_: &Theme| container::Style {
                    background: Some(t::RAISED.into()),
                    border: iced::Border {
                        radius: t::R_BUTTON.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }
    let saved = library().is_saved(&w.video.id);
    r = r.push(
        button(txt(if saved { "✓ Saved" } else { "+ Watch Later" }, 14.0, 20.0).font(MEDIUM))
            .padding([8, 16])
            .style(t::pill)
            .on_press(Message::ToggleLater),
    );
    container(column![txt(title, 20.0, 28.0).font(MEDIUM), r].spacing(12))
        .padding(Padding {
            top: 12.0,
            bottom: 12.0,
            ..Padding::ZERO
        })
        .into()
}

fn description<'a>(info: &'a nt_core::WatchInfo, open: bool) -> El<'a> {
    let head: Vec<&str> = [info.views.as_deref(), info.date.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let body: El<'a> = if open {
        txt(info.description.as_str(), 14.0, 20.0)
            .width(Fill)
            .into()
    } else {
        clamp(info.description.as_str(), 14.0, 20.0, 3).into()
    };
    let mut col = column![txt(head.join("  "), 14.0, 20.0).font(t::BOLD), body].spacing(6);
    if !info.description.is_empty() {
        col = col.push(
            button(txt(if open { "Show less" } else { "Show more" }, 14.0, 20.0).font(MEDIUM))
                .padding(0)
                .style(t::bare)
                .on_press(Message::ToggleDesc),
        );
    }
    container(container(col).width(Fill).padding(12).style(t::panel))
        .padding(Padding {
            bottom: 4.0,
            ..Padding::ZERO
        })
        .into()
}

fn comment<'a>(app: &'a App, c: &'a Comment) -> El<'a> {
    let av = thumb(
        c.author_avatar
            .as_ref()
            .and_then(|u| app.images.get(u, 40.0 * app.scale)),
        40.0,
        40.0,
        20.0,
    );
    let mut head = c.author.clone();
    if let Some(p) = &c.published {
        head.push_str(" • ");
        head.push_str(p);
    }
    let mut foot = String::new();
    if let Some(l) = &c.likes {
        foot.push_str("👍 ");
        foot.push_str(l);
    }
    if let Some(r) = &c.reply_count {
        if !foot.is_empty() {
            foot.push_str("   ");
        }
        foot.push_str(r);
        foot.push_str(if r == "1" { " reply" } else { " replies" });
    }
    let col = column![
        txt(head, 12.0, 20.0).color(t::TEXT_2).font(MEDIUM),
        txt(c.text.as_str(), 14.0, 20.0)
            .width(Fill)
            .wrapping(Wrapping::WordOrGlyph),
        txt(foot, 12.0, 16.0).color(t::TEXT_2),
    ]
    .spacing(4);
    container(row![av, col].spacing(16))
        .padding(Padding {
            bottom: 16.0,
            ..Padding::ZERO
        })
        .into()
}
