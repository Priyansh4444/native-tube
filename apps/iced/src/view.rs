//! Views. Every page body is one `Scroller`; `view` builds only the items that intersect the
//! band [offset - viewport, offset + 2 viewports] (one viewport of overscan each side, plus the
//! half viewport the scroller lets the offset drift before it asks for a rebuild). Layout
//! follows docs/frontend-spec-v2.md, "Look and feel".

use crate::app::{
    App, Body, CardMenu, Channel, Comments, Feed, ListRef, Load, Message, More, Root, Watch,
    id_hash,
};
use crate::icons::{Icon, icon};
use crate::player_view;
use crate::scroller::{Item, Lane, ScrollModel, Scroller, frame_hook};
use crate::theme::{self as t, RAIL, TOP_BAR};
use crate::widgets::{Seg, chip_bar, clamp, highlight, press_hint, rich, search_box, seg, thumb};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
    Space, button, column, container, hover, mouse_area, pin, row, stack, text, text_input,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Pixels, Theme};
use nt_core::{Comment, TOPICS, Video, library};

pub type El<'a> = Element<'a, Message>;

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
const K_TABS: u64 = 15;
const K_SKEL: u64 = 16;

pub fn key(lane: u64, kind: u64, idx: usize) -> u64 {
    (lane << 56) | (kind << 40) | (idx as u64 & 0xff_ffff_ffff)
}

/// Collects one lane's items, building elements only for those inside the band.
pub struct LaneB<'a> {
    model: &'a ScrollModel,
    lane: u64,
    x: f32,
    width: f32,
    lo: f32,
    hi: f32,
    pub y: f32,
    top: Option<f32>,
    items: Vec<Item<'a, Message>>,
}

impl<'a> LaneB<'a> {
    pub fn new(model: &'a ScrollModel, lane: u64, x: f32, width: f32, g: &Geo, start: f32) -> Self {
        LaneB {
            model,
            lane,
            x,
            width: width.max(1.0),
            lo: g.offset - g.vh,
            hi: g.offset + 2.0 * g.vh,
            y: start,
            top: None,
            items: Vec::new(),
        }
    }

    fn visible(&self, h: f32) -> bool {
        self.y + h >= self.lo && self.y <= self.hi
    }

    pub fn item(&mut self, kind: u64, idx: usize, estimate: f32, build: impl FnOnce() -> El<'a>) {
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

    /// Like `item`, but the element is always built (the player, whose visibility matters).
    pub fn contains(&self, kind: u64, idx: usize) -> bool {
        let k = key(self.lane, kind, idx);
        self.items.iter().any(|i| i.key == k)
    }

    /// `n` rows of a fixed height: O(visible) whatever `n` is.
    fn rows(&mut self, kind: u64, n: usize, row_h: f32, mut build: impl FnMut(usize) -> El<'a>) {
        if n == 0 || row_h <= 0.0 {
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

    /// Variable-height items with cached prefix sums: binary search for the first visible one.
    fn var_rows(&mut self, kind: u64, prefix: &[f32], mut build: impl FnMut(usize) -> El<'a>) {
        let n = prefix.len().saturating_sub(1);
        let base = self.y;
        let start = prefix
            .partition_point(|p| base + p < self.lo)
            .saturating_sub(1);
        for i in start..n {
            let (Some(a), Some(b)) = (prefix.get(i), prefix.get(i + 1)) else {
                break;
            };
            let y = base + a;
            if y > self.hi {
                break;
            }
            self.top.get_or_insert(y);
            self.items.push(Item {
                key: key(self.lane, kind, i),
                assumed: b - a,
                element: build(i),
            });
        }
        self.y = base + prefix.last().copied().unwrap_or(0.0);
    }

    pub fn finish(self) -> Lane<'a, Message> {
        Lane {
            x: self.x,
            width: self.width,
            top: self.top.unwrap_or(0.0),
            assumed_total: self.y,
            items: self.items,
        }
    }
}

pub fn txt<'a>(
    s: impl text::IntoFragment<'a>,
    style: t::Style,
) -> text::Text<'a, Theme, iced::Renderer> {
    text(s)
        .size(style.size)
        .line_height(LineHeight::Absolute(Pixels(style.line)))
        .font(style.font)
        .shaping(iced::widget::text::Shaping::Advanced)
}

fn badge_text(v: &Video) -> Option<String> {
    if v.is_live {
        Some("LIVE".to_string())
    } else {
        v.duration.clone()
    }
}

/// Window geometry shared by the page builders.
pub struct Geo {
    pub w: f32,
    pub h: f32,
    /// Width of the body (window minus the rail).
    pub body_w: f32,
    /// Height of the scroller.
    pub vh: f32,
    pub offset: f32,
    /// Content padding: 24, or 16 below 600 px.
    pub pad: f32,
}

pub fn rail_shown(app: &App) -> bool {
    app.win.width >= 792.0
        && !app.fullscreen
        && !matches!(app.top().map(|p| &p.body), Some(Body::Watch(_)))
}

// ---------------------------------------------------------------------------------------------

pub fn view(app: &App) -> El<'_> {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| crate::scroller::trace_event("first_view"));
    app.images.begin_view();
    if app.fullscreen && matches!(app.top().map(|p| &p.body), Some(Body::Watch(_))) {
        return frame_hook(
            container(page(app))
                .style(t::black(0.0))
                .width(Fill)
                .height(Fill),
        );
    }
    let body: El<'_> = container(page(app)).width(Fill).height(Fill).into();
    let main_row: El<'_> = if rail_shown(app) {
        row![rail(app), body].height(Fill).into()
    } else {
        body
    };
    let main = column![top_bar(app), main_row];
    let overlay = stack![
        Space::new().width(Fill).height(Fill),
        dropdown_layer(app),
        card_menu_layer(app),
        toast_layer(app)
    ]
    .width(Fill)
    .height(Fill);
    frame_hook(
        container(stack![main, overlay])
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

/// Widths of the top bar's left block and right spacer, so the search field stays centred
/// and the dropdown can be laid out with the same skeleton.
fn bar_sides(app: &App) -> (f32, f32) {
    if app.win.width >= 900.0 {
        (190.0, 190.0)
    } else {
        (170.0, 16.0)
    }
}

fn search_collapsed(app: &App) -> bool {
    app.win.width < 656.0
}

fn search_field(app: &App) -> El<'_> {
    let focused = app.search_focused;
    let input = text_input("Search", &app.query)
        .id(crate::app::SEARCH_ID)
        .on_input(Message::SearchInput)
        .on_submit(Message::SearchSubmit)
        .size(16)
        .font(t::REGULAR)
        .padding(Padding {
            top: 9.0,
            bottom: 9.0,
            left: if focused { 52.0 } else { 16.0 },
            right: 12.0,
        })
        .style(t::search_input)
        .width(Fill);
    let field = search_box(
        input,
        focused,
        Message::SearchFocus,
        Message::SuggestNav,
        app.sugg_open.then_some(Message::SuggestClose),
    )
    .blur(app.blur);
    // The focused field's search icon is a layer over the input's left padding. Changing the
    // layout around the input when it gains focus (an icon slot growing from 0 px) made iced
    // drop the input's focus on the next rebuild, so only the padding changes.
    let lead: El<'_> = if focused {
        container(icon(Icon::Search, 20.0, t::TEXT))
            .height(38)
            .padding(Padding {
                left: 16.0,
                ..Padding::ZERO
            })
            .align_y(Alignment::Center)
            .into()
    } else {
        Space::new().into()
    };
    let inner = stack![field, lead].height(38);
    let frame = container(inner)
        .style(t::search_frame(focused))
        .height(40)
        .width(Fill)
        .padding(Padding {
            top: 1.0,
            bottom: 1.0,
            left: 1.0,
            right: 0.0,
        });
    let go = button(container(icon(Icon::Search, 24.0, t::TEXT)).center(Fill))
        .width(64)
        .height(40)
        .padding(0)
        .style(t::search_button)
        .on_press(Message::SearchSubmit);
    row![frame, go].height(40).align_y(Alignment::Center).into()
}

fn icon_button<'a>(i: Icon, size: f32, msg: Option<Message>) -> El<'a> {
    let color = if msg.is_some() {
        t::TEXT
    } else {
        Color::from_rgb8(0x55, 0x55, 0x55)
    };
    button(container(icon(i, 24.0, color)).center(Fill))
        .width(size)
        .height(size)
        .padding(0)
        .style(t::icon_button)
        .on_press_maybe(msg)
        .into()
}

fn top_bar(app: &App) -> El<'_> {
    let pad = Padding {
        left: 16.0,
        right: 16.0,
        top: 0.0,
        bottom: 0.0,
    };
    if search_collapsed(app) && app.search_open {
        return row![
            icon_button(Icon::ArrowBack, 40.0, Some(Message::SearchOpen(false))),
            Space::new().width(8),
            search_field(app),
        ]
        .height(TOP_BAR)
        .padding(pad)
        .align_y(Alignment::Center)
        .into();
    }
    let back = icon_button(
        Icon::ArrowBack,
        40.0,
        (app.pages.len() > 1).then_some(Message::Back),
    );
    let left = row![back, txt("NativeTube", t::HEADING)]
        .spacing(12)
        .align_y(Alignment::Center);
    if search_collapsed(app) {
        return row![
            left,
            Space::new().width(Fill),
            icon_button(Icon::Search, 40.0, Some(Message::SearchOpen(true))),
        ]
        .height(TOP_BAR)
        .padding(pad)
        .align_y(Alignment::Center)
        .into();
    }
    let (lw, rw) = bar_sides(app);
    row![
        container(left).width(lw),
        Space::new().width(FillPortion(1)),
        container(search_field(app))
            .width(FillPortion(8))
            .max_width(640),
        Space::new().width(FillPortion(1)),
        Space::new().width(rw),
    ]
    .height(TOP_BAR)
    .padding(pad)
    .align_y(Alignment::Center)
    .into()
}

fn dropdown_layer(app: &App) -> El<'_> {
    if !(app.sugg_open && !app.suggestions.is_empty()) {
        return Space::new().into();
    }
    let typed = app.sugg_for.as_str();
    let items = app.suggestions.iter().enumerate().map(|(i, s)| {
        // The typed prefix at 400, the completion at 500.
        let segs: Vec<Seg<'_>> = match s.to_lowercase().strip_prefix(typed) {
            Some(_) if !typed.is_empty() && s.is_char_boundary(typed.len()) => {
                let (a, b) = s.split_at(typed.len());
                vec![seg(a), seg(b).font(t::MEDIUM)]
            }
            _ => vec![seg(s.as_str()).font(t::MEDIUM)],
        };
        button(
            row![
                icon(Icon::Search, 20.0, t::TEXT),
                rich(segs, t::SUGGEST, 1).color(t::TEXT)
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .height(32)
        .padding(Padding {
            top: 5.0,
            bottom: 5.0,
            left: 16.0,
            right: 16.0,
        })
        .style(t::menu_row(app.sugg_sel == Some(i)))
        .on_press(Message::SuggestPick(i))
        .into()
    });
    let list = container(column(items))
        .style(t::dropdown)
        .padding(Padding {
            top: 16.0,
            bottom: 16.0,
            left: 0.0,
            right: 0.0,
        })
        .width(Fill);
    let pad = Padding {
        left: 16.0,
        right: 16.0,
        top: 0.0,
        bottom: 0.0,
    };
    let positioned: El<'_> = if search_collapsed(app) {
        row![Space::new().width(48), list, Space::new().width(64)]
            .padding(pad)
            .into()
    } else {
        let (lw, rw) = bar_sides(app);
        row![
            Space::new().width(lw),
            Space::new().width(FillPortion(1)),
            container(row![list, Space::new().width(64)])
                .width(FillPortion(8))
                .max_width(640),
            Space::new().width(FillPortion(1)),
            Space::new().width(rw),
        ]
        .padding(pad)
        .into()
    };
    column![Space::new().height(TOP_BAR - 4.0), positioned].into()
}

fn card_menu_layer(app: &App) -> El<'_> {
    let Some(CardMenu {
        list,
        index,
        at,
        context,
        ..
    }) = app.card_menu.clone()
    else {
        return Space::new().into();
    };
    let video = app.list(list).get(index);
    let saved = video.is_some_and(|v| library().is_saved(&v.id));
    let on_that_channel =
        |id: &str| matches!(app.top().map(|p| &p.body), Some(Body::Channel(c)) if c.id == id);
    let has_channel = video
        .and_then(|v| v.channel_id.as_deref())
        .is_some_and(|id| !on_that_channel(id));
    let row_btn = |i: Icon, label: &'static str, msg: Message| -> El<'_> {
        button(
            row![icon(i, 24.0, t::TEXT), txt(label, t::BODY)]
                .spacing(16)
                .align_y(Alignment::Center),
        )
        .width(Fill)
        .height(36)
        .padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 16.0,
            right: 16.0,
        })
        .style(t::menu_row(false))
        .on_press(msg)
        .into()
    };
    let mut items = vec![
        row_btn(
            if saved { Icon::Check } else { Icon::Schedule },
            if saved {
                "Remove from Watch later"
            } else {
                "Add to Watch later"
            },
            Message::CardSaveLater,
        ),
        row_btn(Icon::Share, "Copy link", Message::CardCopyLink),
    ];
    if has_channel {
        items.push(row_btn(
            Icon::VideoLibrary,
            "Open channel",
            Message::CardOpenChannel,
        ));
    }
    if !matches!(
        app.top().map(|p| &p.body),
        Some(Body::History(_) | Body::Later(_))
    ) {
        items.push(row_btn(
            Icon::Close,
            "Not interested",
            Message::CardNotInterested,
        ));
    }
    let (mw, mh) = (260.0, items.len() as f32 * 36.0 + 16.0);
    let menu = container(column(items))
        .style(t::menu)
        .padding(Padding {
            top: 8.0,
            bottom: 8.0,
            left: 0.0,
            right: 0.0,
        })
        .width(mw);
    // At the pointer for a right-click, to the left of the "⋮" button otherwise; flipped and
    // clamped so the whole menu stays in the window.
    let (w, h) = (app.win.width, app.win.height);
    let mut x = if context { at.x } else { at.x - mw };
    if x + mw > w - 8.0 {
        x = at.x - mw;
    }
    let mut y = if context { at.y } else { at.y + 8.0 };
    if y + mh > h - 8.0 {
        y = at.y - mh;
    }
    let x = x.clamp(8.0, (w - mw - 8.0).max(8.0));
    let y = y.clamp(8.0, (h - mh - 8.0).max(8.0));
    stack![
        mouse_area(Space::new().width(Fill).height(Fill))
            .on_press(Message::CardMenuClose)
            .on_right_press(Message::CardMenuClose)
            .on_middle_press(Message::CardMenuClose)
            .on_scroll(|_| Message::CardMenuClose),
        pin(menu).x(x).y(y),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

fn toast_layer(app: &App) -> El<'_> {
    let Some((msg, _)) = &app.toast else {
        return Space::new().into();
    };
    let card = container(txt(msg.as_str(), t::BODY))
        .style(t::dropdown)
        .padding(Padding {
            top: 14.0,
            bottom: 14.0,
            left: 16.0,
            right: 16.0,
        });
    container(card)
        .width(Fill)
        .height(Fill)
        .align_y(Alignment::End)
        .padding(24)
        .into()
}

fn rail(app: &App) -> El<'_> {
    let shown = app.shown_root();
    let entries = [
        (Root::Home, Icon::Home, Icon::HomeFill, "Home"),
        (
            Root::Subscriptions,
            Icon::Subscriptions,
            Icon::SubscriptionsFill,
            "Subscriptions",
        ),
        (Root::History, Icon::History, Icon::HistoryFill, "History"),
        (
            Root::WatchLater,
            Icon::Schedule,
            Icon::ScheduleFill,
            "Watch Later",
        ),
    ];
    let col = column(entries.into_iter().map(|(r, outline, filled, label)| {
        let selected = shown == Some(r);
        button(
            column![
                icon(if selected { filled } else { outline }, 24.0, t::TEXT),
                txt(label, t::RAIL_LABEL)
                    .wrapping(Wrapping::None)
                    .align_x(Alignment::Center)
                    .width(Fill)
            ]
            .align_x(Alignment::Center)
            .spacing(6),
        )
        .width(64)
        .height(74)
        .padding(Padding {
            top: 16.0,
            bottom: 0.0,
            left: 0.0,
            right: 0.0,
        })
        .style(t::rail_entry)
        .on_press(Message::Nav(r))
        .into()
    }));
    container(col)
        .width(RAIL)
        .height(Fill)
        .padding(Padding {
            top: 4.0,
            bottom: 0.0,
            left: 4.0,
            right: 4.0,
        })
        .into()
}

// ---------------------------------------------------------------------------------------------

fn page(app: &App) -> El<'_> {
    let Some(p) = app.top() else {
        return Space::new().into();
    };
    let rail_w = if rail_shown(app) { RAIL } else { 0.0 };
    let body_w = (app.win.width - rail_w).max(100.0);
    let top = if app.fullscreen { 0.0 } else { TOP_BAR };
    let chips = matches!(p.body, Body::Home(_));
    let vh = (app.win.height - top - if chips { t::CHIP_BAR } else { 0.0 }).max(100.0);
    let g = Geo {
        w: app.win.width,
        h: app.win.height,
        body_w,
        vh,
        offset: p.scroll.mark_built(),
        pad: if app.win.width < 600.0 { 16.0 } else { 24.0 },
    };
    let model = &p.scroll;
    let mut on_key: Option<crate::scroller::KeyHandler<'_, Message>> = None;
    let lanes = match &p.body {
        Body::Home(h) => {
            let mut l = LaneB::new(model, 0, g.pad, g.body_w - 2.0 * g.pad, &g, 12.0);
            feed_grid(app, &mut l, &h.load, &h.feed, &g, true, None);
            vec![l.finish()]
        }
        Body::Search(s) => {
            let (x, w) = centered(&g, 1096.0);
            let mut l = LaneB::new(model, 0, x, w, &g, 16.0);
            match &s.load {
                Load::Ready if s.feed.items.is_empty() => l.item(K_STATUS, 0, g.vh - 40.0, || {
                    empty_state(
                        Icon::Search,
                        "No results found",
                        format!("Try different keywords than \u{201c}{}\u{201d}.", s.query),
                        None,
                        g.vh - 40.0,
                    )
                }),
                Load::Ready => {
                    let terms: Vec<String> = s
                        .query
                        .split_whitespace()
                        .map(|t| t.to_lowercase())
                        .collect();
                    list_rows(app, &mut l, &s.feed.items, w, Some(terms));
                    trailer(&mut l, &s.feed, w, false);
                }
                Load::Loading => skeleton_list(&mut l, w),
                Load::Failed(e) => error_item(&mut l, e, Message::Retry, g.vh - 40.0),
            }
            vec![l.finish()]
        }
        Body::Channel(c) => channel(app, c, model, &g),
        Body::Subs(s) => {
            let mut l = LaneB::new(model, 0, g.pad, g.body_w - 2.0 * g.pad, &g, 24.0);
            if s.channels.is_empty() {
                l.item(K_STATUS, 0, g.vh - 48.0, || {
                    empty_state(
                        Icon::Subscriptions,
                        "No subscriptions yet",
                        "Subscribe to channels from a video or channel page and their new videos show up here.".to_string(),
                        Some(("Browse videos", Message::Nav(Root::Home))),
                        g.vh - 48.0,
                    )
                });
            } else {
                l.item(K_STRIP, 0, 140.0, || strip(app, s));
                l.item(K_HEAD, 0, 52.0, || section("Latest"));
                feed_grid(app, &mut l, &s.load, &s.feed, &g, true, None);
            }
            vec![l.finish()]
        }
        Body::History(items) | Body::Later(items) => {
            let history = matches!(p.body, Body::History(_));
            let (x, w) = centered(&g, 1096.0);
            let mut l = LaneB::new(model, 0, x, w, &g, 24.0);
            l.item(K_HEAD, 0, 76.0, || {
                let title = txt(
                    if history {
                        "Watch history"
                    } else {
                        "Watch Later"
                    },
                    t::CHANNEL_NAME,
                );
                let mut r = row![title, Space::new().width(Fill)].align_y(Alignment::Center);
                if history && !items.is_empty() {
                    r = r.push(
                        button(
                            row![
                                icon(Icon::Delete, 24.0, t::TEXT),
                                txt("Clear all watch history", t::BODY_M)
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        )
                        .height(36)
                        .padding(Padding {
                            top: 6.0,
                            bottom: 6.0,
                            left: 12.0,
                            right: 16.0,
                        })
                        .style(t::pill)
                        .on_press(Message::ClearHistory),
                    );
                }
                container(r)
                    .height(76)
                    .padding(Padding {
                        bottom: 24.0,
                        ..Padding::ZERO
                    })
                    .into()
            });
            if items.is_empty() {
                let (ic, title, msg) = if history {
                    (
                        Icon::History,
                        "Your watch history is empty",
                        "Videos you watch will show up here.",
                    )
                } else {
                    (
                        Icon::Schedule,
                        "Nothing saved yet",
                        "Save videos to watch later and they show up here.",
                    )
                };
                l.item(K_STATUS, 0, g.vh - 124.0, || {
                    empty_state(
                        ic,
                        title,
                        msg.to_string(),
                        Some(("Browse videos", Message::Nav(Root::Home))),
                        g.vh - 124.0,
                    )
                });
            } else {
                list_rows(app, &mut l, items, w, None);
            }
            vec![l.finish()]
        }
        Body::Watch(w) => {
            on_key = Some(Box::new(App::watch_key_message));
            watch(app, p.id, w, model, &g)
        }
    };
    let scroller = Scroller::new(model, lanes, Message::ScrollMoved)
        .on_key(on_key)
        .keys_enabled(!app.search_focused)
        .scale(app.scale);
    match &p.body {
        Body::Home(h) => column![
            container(chips_row(h)).padding(Padding {
                left: g.pad,
                right: g.pad,
                top: 0.0,
                bottom: 0.0,
            }),
            scroller
        ]
        .into(),
        _ => scroller.into(),
    }
}

/// Centered content of at most `max` px: (x, width).
fn centered(g: &Geo, max: f32) -> (f32, f32) {
    let w = (g.body_w - 2.0 * g.pad).min(max).max(1.0);
    (((g.body_w - w) / 2.0).max(g.pad), w)
}

fn section(label: &str) -> El<'_> {
    container(txt(label, t::HEADING))
        .height(52)
        .padding(Padding {
            top: 0.0,
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}

/// The spec's empty state: a 120 px icon, a 20/28 title, a line of secondary text and an
/// optional action, centred in a box of a fixed height (fixed so the scroller can lay it out).
fn empty_state<'a>(
    i: Icon,
    title: &'a str,
    body: String,
    action: Option<(&'a str, Message)>,
    height: f32,
) -> El<'a> {
    let mut col = column![
        icon(i, 120.0, t::TEXT_2),
        txt(title, t::HEADING),
        txt(body, t::BODY)
            .color(t::TEXT_2)
            .align_x(Alignment::Center),
    ]
    .spacing(12)
    .align_x(Alignment::Center)
    .max_width(520);
    if let Some((label, msg)) = action {
        col = col.push(Space::new().height(4)).push(
            button(txt(label, t::BODY_M))
                .padding(Padding {
                    top: 8.0,
                    bottom: 8.0,
                    left: 16.0,
                    right: 16.0,
                })
                .style(t::pill)
                .on_press(msg),
        );
    }
    container(col)
        .width(Fill)
        .height(Length::Fixed(height.max(300.0)))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

pub fn error_box<'a>(msg: &str, retry: Message, height: f32) -> El<'a> {
    let network = msg.starts_with("Couldn't reach");
    empty_state(
        if network { Icon::WifiOff } else { Icon::Error },
        "Something went wrong",
        msg.to_string(),
        Some(("Retry", retry)),
        height,
    )
}

fn error_item(l: &mut LaneB<'_>, msg: &str, retry: Message, height: f32) {
    let msg = msg.to_string();
    let h = height.max(300.0);
    l.item(K_STATUS, 1, h, move || error_box(&msg, retry, h));
}

fn trailer(l: &mut LaneB<'_>, feed: &Feed, width: f32, grid: bool) {
    match &feed.more {
        Some(More::Loading) => {
            if grid {
                l.item(K_TRAIL, 0, 72.0, || {
                    container(crate::widgets::spinner(32.0))
                        .width(Fill)
                        .height(72)
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center)
                        .into()
                });
            } else {
                let (tw, th) = list_geom(width);
                l.item(K_TRAIL, 0, th + 16.0, move || skeleton_row(tw, th));
            }
        }
        Some(More::Failed(e)) => {
            let e = e.clone();
            l.item(K_TRAIL, 1, 300.0, move || {
                error_box(&e, Message::RetryMore, 300.0)
            })
        }
        _ => {}
    }
}

fn chips_row(h: &crate::app::Home) -> El<'_> {
    let chips: Vec<El<'_>> = TOPICS
        .iter()
        .enumerate()
        .map(|(i, topic)| {
            button(txt(topic.title, t::BODY_M))
                .padding(Padding {
                    top: 6.0,
                    bottom: 6.0,
                    left: 12.0,
                    right: 12.0,
                })
                .height(32)
                .style(t::chip(i == h.topic))
                .on_press(Message::Topic(i))
                .into()
        })
        .collect();
    chip_bar(chips, &h.chip_offset).into()
}

/// Columns by width (spec: 1 below 600, 2 below 900, 3 below 1600, 4 below 2300, else 5), and
/// the card width that fills the row exactly.
fn grid_geom(body_w: f32, lane_w: f32) -> (usize, f32) {
    let cols = match body_w {
        w if w < 600.0 => 1,
        w if w < 900.0 => 2,
        w if w < 1600.0 => 3,
        w if w < 2300.0 => 4,
        _ => 5,
    };
    let w = (lane_w - (cols - 1) as f32 * 16.0) / cols as f32;
    (cols, w.floor().max(1.0))
}

/// One grid row: the card's thumbnail, 12 px, two title lines at most, the channel and meta
/// lines (or only meta on a channel page), and the 40 px row gap.
fn card_height(w: f32, with_channel: bool) -> f32 {
    (w * 9.0 / 16.0).round() + 12.0 + 44.0 + if with_channel { 40.0 } else { 20.0 }
}

fn feed_grid<'a>(
    app: &'a App,
    l: &mut LaneB<'a>,
    load: &'a Load,
    feed: &'a Feed,
    g: &Geo,
    with_channel: bool,
    empty: Option<&'static str>,
) {
    let lane_w = l.width;
    let (cols, w) = grid_geom(g.body_w, lane_w);
    let row_h = card_height(w, with_channel) + 40.0;
    match load {
        Load::Ready if feed.items.is_empty() => l.item(K_STATUS, 2, 300.0, move || {
            empty_state(
                Icon::VideoLibrary,
                empty.unwrap_or("No videos here yet"),
                "Try another topic or come back later.".to_string(),
                None,
                300.0,
            )
        }),
        Load::Ready => {
            let items = &feed.items;
            let n = items.len().div_ceil(cols);
            l.rows(K_ROW, n, row_h, |r| {
                let start = r * cols;
                let end = (start + cols).min(items.len());
                let cards = items
                    .get(start..end)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                    .map(|(k, v)| card(app, v, start + k, ListRef::Main, w, with_channel));
                row(cards).spacing(16).into()
            });
            trailer(l, feed, lane_w, true);
        }
        Load::Loading => {
            let rows = ((g.vh / row_h).ceil() as usize).clamp(1, 4);
            l.rows(K_SKEL, rows, row_h, |_| {
                row((0..cols).map(|_| skeleton_card(w, with_channel)))
                    .spacing(16)
                    .into()
            });
        }
        Load::Failed(e) => error_item(l, e, Message::Retry, g.vh - 40.0),
    }
}

fn skeleton_bar<'a>(w: impl Into<Length>, h: f32, r: f32) -> El<'a> {
    container(Space::new())
        .width(w)
        .height(h)
        .style(t::fill_box(r))
        .into()
}

fn skeleton_card<'a>(w: f32, with_channel: bool) -> El<'a> {
    let th = (w * 9.0 / 16.0).round();
    let lines = column![
        skeleton_bar(Length::FillPortion(9), 20.0, 4.0),
        row![
            skeleton_bar(Length::FillPortion(6), 20.0, 4.0),
            Space::new().width(FillPortion(4))
        ],
    ]
    .spacing(8)
    .width(Fill);
    let mut info = row![].spacing(12);
    if with_channel {
        info = info.push(skeleton_bar(36.0, 36.0, 18.0));
    }
    column![skeleton_bar(w, th, t::R_THUMB), info.push(lines)]
        .spacing(12)
        .width(w)
        .into()
}

fn skeleton_row<'a>(tw: f32, th: f32) -> El<'a> {
    row![
        skeleton_bar(tw, th, t::R_THUMB),
        column![
            skeleton_bar(Length::FillPortion(8), 24.0, 4.0),
            row![
                skeleton_bar(Length::FillPortion(4), 16.0, 4.0),
                Space::new().width(FillPortion(5))
            ],
            row![
                skeleton_bar(Length::FillPortion(3), 16.0, 4.0),
                Space::new().width(FillPortion(6))
            ],
        ]
        .spacing(12)
        .width(Fill)
    ]
    .spacing(16)
    .height(th)
    .into()
}

fn skeleton_list(l: &mut LaneB<'_>, w: f32) {
    let (tw, th) = list_geom(w);
    l.rows(K_SKEL, 4, th + 16.0, move |_| skeleton_row(tw, th));
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

fn channel_name<'a>(v: &'a Video, i: usize, list: ListRef, style: t::Style) -> El<'a> {
    let name = clamp(v.channel.as_str(), style, 1);
    match &v.channel_id {
        Some(_) if !v.channel.is_empty() => button(name)
            .padding(0)
            .style(t::link)
            .on_press(Message::OpenChannelOf(list, i))
            .into(),
        _ => container(name.color(t::TEXT_2)).into(),
    }
}

/// Right-click anywhere on a card or row opens its menu at the pointer (captured, so nothing
/// under the pointer reacts).
fn context_menu<'a>(content: impl Into<El<'a>>, list: ListRef, i: usize) -> El<'a> {
    mouse_area(content)
        .on_right_press(Message::CardContext(list, i))
        .into()
}

/// The "⋮" button shown over a card or row while it is hovered.
fn more_button<'a>(list: ListRef, i: usize) -> El<'a> {
    button(container(icon(Icon::MoreVert, 24.0, t::TEXT)).center(Fill))
        .width(40)
        .height(40)
        .padding(0)
        .style(t::icon_button)
        .on_press(Message::CardMenu(list, i))
        .into()
}

fn card<'a>(
    app: &'a App,
    v: &'a Video,
    i: usize,
    list: ListRef,
    w: f32,
    with_channel: bool,
) -> El<'a> {
    let th = (w * 9.0 / 16.0).round();
    let img = thumb(
        app.images.get(&v.thumbnail, w * app.scale),
        w,
        th,
        t::R_THUMB,
    )
    .cover()
    .badge(badge_text(v), v.is_live)
    .progress(app.prefs.progress(&v.id));
    let hint = Message::Preload(list, i, id_hash(&v.id));
    let img = press_hint(img, hint.clone());
    let mut text_col = column![press_hint(
        clamp(v.title.as_str(), t::CARD_TITLE, 2).color(t::TEXT),
        hint
    )]
    .width(Fill)
    .padding(Padding {
        right: 24.0,
        ..Padding::ZERO
    });
    if with_channel && !v.channel.is_empty() {
        text_col = text_col
            .push(Space::new().height(4))
            .push(channel_name(v, i, list, t::BODY));
    }
    text_col = text_col.push(clamp(v.meta_line(), t::BODY, 1).color(t::TEXT_2));
    let mut info = row![].spacing(12);
    if with_channel && let Some(a) = avatar(app, v, i, list, 36.0) {
        info = info.push(a);
    }
    info = info.push(text_col);
    let base = button(column![img, info].spacing(12))
        .width(w)
        .padding(0)
        .style(t::card)
        .on_press(Message::Open(list, i, id_hash(&v.id)));
    let menu = column![
        Space::new().height(th + 4.0),
        row![Space::new().width(Fill), more_button(list, i)]
    ]
    .width(w);
    context_menu(hover(base, menu), list, i)
}

/// Search-style rows: thumbnail clamp(240, 40% of the content, 500) wide, 16:9.
fn list_geom(content_w: f32) -> (f32, f32) {
    let tw = (content_w * 0.4)
        .clamp(240.0, 500.0)
        .min(content_w * 0.5)
        .floor();
    (tw, (tw * 9.0 / 16.0).round())
}

fn list_rows<'a>(
    app: &'a App,
    l: &mut LaneB<'a>,
    items: &'a [Video],
    content_w: f32,
    terms: Option<Vec<String>>,
) {
    let (tw, th) = list_geom(content_w);
    l.rows(K_ROW, items.len(), th + 16.0, |i| match items.get(i) {
        Some(v) => list_row(app, v, i, ListRef::Main, tw, th, terms.as_deref()),
        None => Space::new().into(),
    });
}

fn list_row<'a>(
    app: &'a App,
    v: &'a Video,
    i: usize,
    list: ListRef,
    tw: f32,
    th: f32,
    terms: Option<&[String]>,
) -> El<'a> {
    let img = thumb(
        app.images.get(&v.thumbnail, tw * app.scale),
        tw,
        th,
        t::R_THUMB,
    )
    .cover()
    .badge(badge_text(v), v.is_live)
    .progress(app.prefs.progress(&v.id));
    let mut chan = row![].spacing(8).align_y(Alignment::Center);
    if let Some(a) = avatar(app, v, i, list, 24.0) {
        chan = chan.push(a);
    }
    chan = chan.push(channel_name(v, i, list, t::META_S));
    let hint = Message::Preload(list, i, id_hash(&v.id));
    let img = press_hint(img, hint.clone());
    let mut col = column![
        press_hint(
            clamp(v.title.as_str(), t::SEARCH_TITLE, 2).color(t::TEXT),
            hint
        ),
        clamp(v.meta_line(), t::META_S, 1).color(t::TEXT_2),
        container(chan).padding(Padding {
            top: 12.0,
            bottom: 12.0,
            ..Padding::ZERO
        }),
    ]
    .width(Fill)
    .padding(Padding {
        right: 32.0,
        ..Padding::ZERO
    });
    if let Some(s) = v.snippet.as_deref().filter(|s| !s.is_empty()) {
        let segs = match terms {
            Some(terms) => highlight(s, terms, t::MEDIUM, t::TEXT),
            None => vec![seg(s)],
        };
        col = col.push(rich(segs, t::META_S, 1).color(t::TEXT_2));
    }
    let base = button(row![img, col].spacing(16))
        .width(Fill)
        .height(th)
        .padding(0)
        .style(t::card)
        .on_press(Message::Open(list, i, id_hash(&v.id)));
    let menu = row![Space::new().width(Fill), more_button(list, i)];
    context_menu(hover(base, menu), list, i)
}

pub fn compact_row<'a>(app: &'a App, v: &'a Video, i: usize) -> El<'a> {
    let img = thumb(
        app.images.get(&v.thumbnail, 168.0 * app.scale),
        168.0,
        94.0,
        t::R_COMPACT,
    )
    .cover()
    .badge(badge_text(v), v.is_live)
    .progress(app.prefs.progress(&v.id));
    let col = column![
        clamp(v.title.as_str(), t::BODY_M, 2).color(t::TEXT),
        Space::new().height(4),
        clamp(v.channel.as_str(), t::META_S, 1).color(t::TEXT_2),
        clamp(v.meta_line(), t::META_S, 1).color(t::TEXT_2),
    ]
    .width(Fill)
    .padding(Padding {
        right: 24.0,
        ..Padding::ZERO
    });
    let base = button(row![img, col].spacing(8))
        .width(Fill)
        .height(94)
        .padding(0)
        .style(t::card)
        .on_press(Message::Open(ListRef::Related, i, id_hash(&v.id)));
    let menu = row![Space::new().width(Fill), more_button(ListRef::Related, i)].height(40);
    context_menu(hover(base, column![menu]), ListRef::Related, i)
}

pub fn subscribe_button<'a>(subscribed: bool) -> El<'a> {
    if subscribed {
        button(
            row![
                icon(Icon::Notifications, 24.0, t::TEXT),
                txt("Subscribed", t::BODY_M)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .height(36)
        .padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 12.0,
            right: 16.0,
        })
        .style(t::pill)
        .on_press(Message::ToggleSubscribe)
        .into()
    } else {
        button(txt("Subscribe", t::BODY_M))
            .height(36)
            .padding(Padding {
                top: 8.0,
                bottom: 8.0,
                left: 16.0,
                right: 16.0,
            })
            .style(t::primary)
            .on_press(Message::ToggleSubscribe)
            .into()
    }
}

fn channel<'a>(
    app: &'a App,
    c: &'a Channel,
    model: &'a ScrollModel,
    g: &Geo,
) -> Vec<Lane<'a, Message>> {
    let (x, w) = centered(g, 1284.0);
    let mut l = LaneB::new(model, 0, x, w, g, 0.0);
    if let Some(banner) = c.info.as_ref().and_then(|i| i.banner.as_ref()) {
        let bh = (w / 6.2).round();
        l.item(K_BANNER, 0, bh + 16.0, || {
            container(thumb(app.images.get(banner, w * app.scale), w, bh, 16.0).cover())
                .padding(Padding {
                    bottom: 16.0,
                    ..Padding::ZERO
                })
                .into()
        });
    } else {
        l.y += 16.0;
    }
    l.item(K_HEAD, 0, 200.0, || channel_header(app, c, g));
    l.item(K_TABS, 0, 49.0, || {
        column![
            row![
                column![
                    container(txt("Videos", t::TAB))
                        .height(46)
                        .align_y(Alignment::Center),
                    container(Space::new())
                        .width(Fill)
                        .height(2)
                        .style(t::underline),
                ]
                .width(Length::Shrink)
            ],
            container(Space::new())
                .width(Fill)
                .height(1)
                .style(t::divider),
        ]
        .into()
    });
    l.y += 24.0;
    feed_grid(
        app,
        &mut l,
        &c.load,
        &c.feed,
        g,
        false,
        Some("This channel has no videos"),
    );
    vec![l.finish()]
}

fn channel_header<'a>(app: &'a App, c: &'a Channel, g: &Geo) -> El<'a> {
    let (name, avatar_url, desc) = match &c.info {
        Some(i) => (i.name.as_str(), i.avatar.as_ref(), i.description.as_str()),
        None => (c.stub.name.as_str(), c.stub.avatar.as_ref(), ""),
    };
    let line: String = c
        .info
        .as_ref()
        .map(|i| {
            [
                i.handle.as_deref(),
                i.subscribers.as_deref(),
                i.video_count.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" • ")
        })
        .unwrap_or_default();
    let size = if g.w < 600.0 { 72.0 } else { 160.0 };
    let av = thumb(
        avatar_url.and_then(|u| app.images.get(u, size * app.scale)),
        size,
        size,
        size / 2.0,
    );
    let mut col = column![txt(name, t::CHANNEL_NAME)].spacing(8).width(Fill);
    if !line.is_empty() {
        col = col.push(clamp(line, t::BODY, 1).color(t::TEXT_2));
    }
    if !desc.is_empty() {
        let d = if c.desc_open {
            clamp(desc, t::BODY, 0)
                .color(t::TEXT_2)
                .on_press(Message::ToggleDesc)
        } else {
            clamp(desc, t::BODY, 1)
                .color(t::TEXT_2)
                .suffix("…more", t::MEDIUM, t::TEXT)
                .on_press(Message::ToggleDesc)
        };
        col = col.push(d);
    }
    if c.info.is_some() {
        col = col
            .push(Space::new().height(4))
            .push(subscribe_button(app.is_subscribed(&c.id)));
    } else if let Load::Failed(e) = &c.load {
        col = col.push(error_box(e, Message::Retry, 300.0));
    }
    container(row![av, col].spacing(24).align_y(Alignment::Center))
        .padding(Padding {
            top: 8.0,
            bottom: 16.0,
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
            column![av, clamp(c.name.as_str(), t::META_S, 1).color(t::TEXT)]
                .spacing(8)
                .align_x(Alignment::Center),
        )
        .width(96)
        .padding(4)
        .style(t::card)
        .on_press(Message::OpenChannel(c.clone()))
        .into()
    });
    container(row(items).spacing(8).wrap().vertical_spacing(8))
        .padding(Padding {
            bottom: 32.0,
            ..Padding::ZERO
        })
        .into()
}

// ---------------------------------------------------------------------------------------------
// Watch page

/// Watch geometry: (primary x, primary width, secondary x and width when two columns).
pub struct WatchGeo {
    pub px: f32,
    pub pw: f32,
    pub secondary: Option<(f32, f32)>,
    pub player_w: f32,
    pub player_h: f32,
    pub theatre: bool,
}

pub fn watch_geo(app: &App) -> WatchGeo {
    let (w, h) = (app.win.width, app.win.height);
    let pmax = ((h - 169.0) * 16.0 / 9.0).max(320.0);
    let theatre = app.prefs.theatre && !app.fullscreen;
    let (px, pw, secondary) = if w >= 1017.0 {
        let avail = w - 72.0;
        let s = (avail * 0.29).clamp(300.0, 402.0);
        let p = (avail - s).min(pmax).max(200.0);
        let block = p + 24.0 + s;
        let left = ((w - block) / 2.0).max(24.0);
        (left, p, Some((left + p + 24.0, s)))
    } else {
        let p = (w - 48.0).min(pmax).max(100.0);
        (
            ((w - p) / 2.0).max(24.0).min(24.0_f32.max((w - p) / 2.0)),
            p,
            None,
        )
    };
    let (player_w, player_h) = if app.fullscreen {
        (w, h)
    } else if theatre {
        (w, (w * 0.5625).min(h - 169.0).max(200.0))
    } else {
        (pw, (pw * 9.0 / 16.0).round())
    };
    WatchGeo {
        px,
        pw,
        secondary,
        player_w,
        player_h,
        theatre,
    }
}

fn watch<'a>(
    app: &'a App,
    page_id: u64,
    w: &'a Watch,
    model: &'a ScrollModel,
    g: &Geo,
) -> Vec<Lane<'a, Message>> {
    let wg = watch_geo(app);
    let playing_here = app.playing_page == Some(page_id);
    if app.fullscreen {
        let mut l = LaneB::new(model, 3, 0.0, g.w, g, 0.0);
        l.item(K_PLAYER, 0, g.h, || {
            player_view::player(app, w, wg.player_w, wg.player_h, 0.0)
        });
        crate::ext::set_video_visible(playing_here);
        return vec![l.finish()];
    }
    let mut lanes = Vec::new();
    let mut top = 24.0;
    let mut player_built = false;
    if wg.theatre {
        let mut l = LaneB::new(model, 3, 0.0, g.w, g, 0.0);
        l.item(K_PLAYER, 0, wg.player_h, || {
            player_view::player(app, w, wg.player_w, wg.player_h, 0.0)
        });
        player_built = l.contains(K_PLAYER, 0);
        lanes.push(l.finish());
        top = wg.player_h;
    }
    let mut l = LaneB::new(
        model,
        0,
        wg.px,
        wg.pw,
        g,
        if wg.theatre { top } else { 24.0 },
    );
    if !wg.theatre {
        l.item(K_PLAYER, 0, wg.player_h, || {
            player_view::player(app, w, wg.player_w, wg.player_h, t::R_THUMB)
        });
        player_built = l.contains(K_PLAYER, 0);
    }
    crate::ext::set_video_visible(playing_here && player_built);
    l.item(K_INFO, 0, 140.0, || player_view::watch_info(app, w, wg.pw));
    match &w.load {
        Load::Ready => {
            if let Some(info) = &w.info {
                l.item(K_DESC, 0, 150.0, || player_view::description(info, w));
            }
            if wg.secondary.is_none() && !w.related.is_empty() {
                l.item(K_RHEAD, 0, 24.0, || Space::new().height(24).into());
                let related = &w.related;
                l.rows(K_REL, related.len(), 102.0, |i| match related.get(i) {
                    Some(v) => compact_row(app, v, i),
                    None => Space::new().into(),
                });
            }
            comments(app, &mut l, w, wg.pw);
        }
        Load::Loading => {
            l.item(K_STATUS, 0, 120.0, || {
                column![
                    skeleton_bar(Length::FillPortion(7), 20.0, 4.0),
                    skeleton_bar(Length::FillPortion(4), 20.0, 4.0),
                    skeleton_bar(Fill, 80.0, 12.0),
                ]
                .spacing(12)
                .into()
            });
        }
        Load::Failed(e) => error_item(&mut l, e, Message::Retry, 320.0),
    }
    lanes.push(l.finish());
    if let Some((sx, sw)) = wg.secondary {
        let mut r = LaneB::new(
            model,
            1,
            sx,
            sw,
            g,
            if wg.theatre { top + 24.0 } else { 24.0 },
        );
        match &w.load {
            Load::Loading => {
                r.rows(K_SKEL, 6, 102.0, |_| {
                    row![
                        skeleton_bar(168.0, 94.0, t::R_COMPACT),
                        column![
                            skeleton_bar(Fill, 16.0, 4.0),
                            skeleton_bar(Length::FillPortion(2), 12.0, 4.0)
                        ]
                        .spacing(8)
                        .width(Fill)
                    ]
                    .spacing(8)
                    .into()
                });
            }
            _ => {
                let related = &w.related;
                r.rows(K_REL, related.len(), 102.0, |i| match related.get(i) {
                    Some(v) => compact_row(app, v, i),
                    None => Space::new().into(),
                });
            }
        }
        lanes.push(r.finish());
    }
    lanes
}

fn comments<'a>(app: &'a App, l: &mut LaneB<'a>, w: &'a Watch, width: f32) {
    w.comments_y.set(l.y);
    l.item(K_CHEAD, 0, 76.0, || {
        let label = match &w.comments_total {
            Some(n) => format!("{n} Comments"),
            None => "Comments".to_string(),
        };
        container(txt(label, t::HEADING))
            .padding(Padding {
                top: 24.0,
                bottom: 24.0,
                ..Padding::ZERO
            })
            .into()
    });
    // Prefix sums of measured (or estimated) heights, rebuilt only when a height changed.
    let text_w = (width - 56.0).max(100.0);
    let version = l.model.heights_version();
    {
        let mut cache = w.comments_layout.borrow_mut();
        if cache.version != version
            || cache.width != width
            || cache.prefix.len() != w.comments.len() + 1
        {
            let mut prefix = Vec::with_capacity(w.comments.len() + 1);
            let mut y = 0.0;
            prefix.push(0.0);
            for (i, chars) in w.comment_chars.iter().enumerate() {
                let h = l.model.height(key(0, K_COMMENT, i)).unwrap_or_else(|| {
                    let lines = ((*chars as f32 * 7.4) / text_w).ceil().clamp(1.0, 4.0);
                    18.0 + 2.0 + lines * 20.0 + 8.0 + 18.0 + 24.0
                });
                y += h;
                prefix.push(y);
            }
            *cache = crate::app::VarList {
                version,
                width,
                prefix,
            };
        }
    }
    let cache = w.comments_layout.borrow();
    let comments = &w.comments;
    l.var_rows(K_COMMENT, &cache.prefix, |i| match comments.get(i) {
        Some(c) => comment(app, c, i, w.comments_open.contains(&i), text_w),
        None => Space::new().into(),
    });
    drop(cache);
    match &w.comments_state {
        Comments::Loading => l.item(K_CTRAIL, 0, 72.0, || {
            container(crate::widgets::spinner(32.0))
                .width(Fill)
                .height(72)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .into()
        }),
        Comments::Failed(e) => {
            let e = e.clone();
            l.item(K_CTRAIL, 1, 300.0, move || {
                error_box(&e, Message::RetryComments, 300.0)
            })
        }
        Comments::NotStarted if w.info.as_ref().is_some_and(|i| i.comments.is_none()) => {
            l.item(K_CTRAIL, 2, 60.0, || {
                container(txt("Comments are turned off.", t::BODY).color(t::TEXT_2))
                    .height(60)
                    .into()
            })
        }
        _ => {}
    }
}

fn comment<'a>(app: &'a App, c: &'a Comment, i: usize, open: bool, text_w: f32) -> El<'a> {
    let av = thumb(
        c.author_avatar
            .as_ref()
            .and_then(|u| app.images.get(u, 40.0 * app.scale)),
        40.0,
        40.0,
        20.0,
    );
    let mut head = row![clamp(c.author.as_str(), t::AUTHOR, 1).color(t::TEXT)]
        .spacing(4)
        .align_y(Alignment::Center);
    if let Some(p) = &c.published {
        head = row![
            container(txt(c.author.as_str(), t::AUTHOR).color(t::TEXT)),
            txt(p.as_str(), t::META_S).color(t::TEXT_2)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
    }
    let body = clamp(c.text.as_str(), t::BODY, if open { 0 } else { 4 }).color(t::TEXT);
    let mut col = column![head, body].spacing(2).width(Fill);
    let est_lines =
        (c.text.chars().count() as f32 * 7.4 / text_w).ceil() + c.text.matches('\n').count() as f32;
    if est_lines > 4.0 {
        col = col.push(
            button(txt(if open { "Show less" } else { "Read more" }, t::BODY_M).color(t::TEXT_2))
                .padding(0)
                .style(t::bare)
                .on_press(Message::ToggleComment(i)),
        );
    }
    let mut foot = row![icon(Icon::ThumbUp, 16.0, t::TEXT_2)]
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(l) = &c.likes {
        foot = foot.push(txt(l.as_str(), t::META_S).color(t::TEXT_2));
    }
    col = col.push(container(foot).padding(Padding {
        top: 8.0,
        ..Padding::ZERO
    }));
    container(row![av, col].spacing(16))
        .padding(Padding {
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}
