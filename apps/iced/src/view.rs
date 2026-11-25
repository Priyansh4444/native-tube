//! Views. Every page body is one `Scroller`; `view` builds only the items that intersect the
//! band [offset - viewport, offset + 2 viewports] (one viewport of overscan each side, plus the
//! half viewport the scroller lets the offset drift before it asks for a rebuild). Layout
//! follows docs/frontend-spec-v2.md, "Look and feel".

use crate::app::{
    App, Body, CardMenu, Channel, Comments, Feed, ListRef, Load, Message, More, Root, Search,
    SearchEntry, Watch, id_hash,
};
use crate::icons::{Icon, icon};
use crate::interaction::{Control, action};
use crate::player_view;
use crate::scroller::{Item, Lane, ScrollModel, Scroller, frame_hook};
use crate::theme::{self as t, RAIL, TOP_BAR};
use crate::widgets::{Seg, chip_bar, clamp, highlight, press_hint, rich, search_box, seg, thumb};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
    Space, button, column, container, hover, mouse_area, pin, row, shader, stack, text, text_input,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Pixels, Theme};
use nt_core::{Comment, SearchNotice, TOPICS, Video, library};

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
const K_MIXED: u64 = 20;
const K_PLAYLIST: u64 = 21;
const K_NOTICE: u64 = 17;
const K_MOMENT: u64 = 18;

pub fn key(lane: u64, kind: u64, idx: usize) -> u64 {
    (lane << 56) | (kind << 40) | (idx as u64 & 0xff_ffff_ffff)
}

pub fn comment_key(index: usize) -> u64 {
    key(0, K_COMMENT, index)
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
    if v.upcoming.is_some() {
        None
    } else if v.is_live {
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

pub fn rail_width(app: &App) -> f32 {
    if app.fullscreen
        || app.win.width < 792.0
        || matches!(app.top().map(|p| &p.body), Some(Body::Watch(_)))
    {
        0.0
    } else if app.prefs.guide {
        240.0
    } else {
        RAIL
    }
}

// ---------------------------------------------------------------------------------------------

pub fn view(app: &App) -> El<'_> {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| crate::scroller::trace_event("first_view"));
    app.images.begin_view();
    let body: El<'_> = container(page(app)).width(Fill).height(Fill).into();
    let main: El<'_> = if app.fullscreen {
        body
    } else {
        let rail_w = rail_width(app);
        let main_row: El<'_> = if rail_w > 0.0 {
            row![rail(app, rail_w > RAIL), body].height(Fill).into()
        } else {
            body
        };
        column![top_bar(app), main_row].into()
    };
    let modal = app.card_menu.is_some()
        || app.player_context.is_some()
        || app.shortcuts
        || app.search_filters
        || app.queue_open
        || app.guide_is_overlay();
    let main = Control::new(main).block_keyboard(modal).element();
    let overlay = stack![
        Space::new().width(Fill).height(Fill),
        dropdown_layer(app),
        guide_layer(app),
        context_menu_layer(app),
        search_filters_layer(app),
        queue_layer(app),
        shortcuts_layer(app),
        toast_layer(app)
    ]
    .width(Fill)
    .height(Fill);
    frame_hook(
        container(stack![main, overlay]).width(Fill).height(Fill),
        app,
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
        (170.0, 40.0)
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
    );
    // The focused field's search icon is a layer over the input's left padding. Changing the
    // layout around the input when it gains focus (an icon slot growing from 0 px) made iced
    // drop the input's focus on the next rebuild, so only the padding changes.
    let glyph = if focused {
        icon(Icon::Search, 20.0, t::TEXT)
    } else {
        Space::new().width(20).height(20).into()
    };
    let lead = container(glyph)
        .height(38)
        .padding(Padding {
            left: 16.0,
            ..Padding::ZERO
        })
        .align_y(Alignment::Center);
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
    row![
        Control::new(frame)
            .target("search_field", "Search")
            .element(),
        action(go, "search_button", "Search", Message::SearchSubmit)
    ]
    .height(40)
    .align_y(Alignment::Center)
    .into()
}

fn icon_button<'a>(i: Icon, size: f32, msg: Option<Message>) -> El<'a> {
    let color = if msg.is_some() {
        t::TEXT
    } else {
        Color::from_rgb8(0x55, 0x55, 0x55)
    };
    let control = button(container(icon(i, 24.0, color)).center(Fill))
        .width(size)
        .height(size)
        .padding(0)
        .style(t::icon_button)
        .on_press_maybe(msg.clone());
    let (kind, label) = match i {
        Icon::ArrowBack => ("back", "Back"),
        Icon::Search => ("search_button", "Search"),
        Icon::Menu => ("guide", "Guide"),
        Icon::VideoLibrary => ("queue", "Queue"),
        _ => ("button", ""),
    };
    match msg {
        Some(msg) => action(control, kind, label, msg),
        None => control.into(),
    }
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
    let guide = icon_button(Icon::Menu, 40.0, Some(Message::Guide));
    let left = row![guide, back, txt("NativeTube", t::HEADING)]
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
    let queue = icon_button(Icon::VideoLibrary, 40.0, Some(Message::Queue(true)));
    row![
        container(left).width(lw),
        Space::new().width(FillPortion(1)),
        container(search_field(app))
            .width(FillPortion(8))
            .max_width(640),
        Space::new().width(FillPortion(1)),
        container(queue).width(rw).align_x(Alignment::End),
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
        let button = button(
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
        .on_press(Message::SuggestPick(i));
        Control::new(button)
            .target("suggestion", s.as_str())
            .element()
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
    let list = Control::new(iced::widget::opaque(list)).cover().element();
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

fn menu_row<'a>(i: Icon, label: &'static str, message: Message) -> El<'a> {
    action(
        button(
            row![icon(i, 24.0, t::TEXT), txt(label, t::BODY)]
                .spacing(16)
                .align_y(Alignment::Center),
        )
        .width(Fill)
        .height(36)
        .padding([6, 16])
        .style(t::menu_row(false))
        .on_press(message.clone()),
        "menu_item",
        label,
        message,
    )
}

fn context_menu_layer(app: &App) -> El<'_> {
    let (at, context, items) = if let Some(at) = app.player_context {
        (
            at,
            true,
            vec![
                menu_row(
                    if app.play.looping {
                        Icon::Check
                    } else {
                        Icon::Replay
                    },
                    "Loop",
                    Message::Loop,
                ),
                menu_row(Icon::Share, "Copy video URL", Message::CopyVideoUrl(false)),
                menu_row(
                    Icon::Schedule,
                    "Copy video URL at current time",
                    Message::CopyVideoUrl(true),
                ),
                menu_row(Icon::Tune, "Stats for nerds", Message::Stats(true)),
            ],
        )
    } else if let Some(CardMenu {
        list,
        index,
        hash,
        at,
        context,
    }) = app.card_menu
    {
        let Some(video) = app.list(list).get(index).filter(|v| id_hash(&v.id) == hash) else {
            return Space::new().into();
        };
        let saved = library().is_saved(&video.id);
        let mut items = vec![
            menu_row(Icon::PlayArrow, "Play", Message::CardPlay),
            menu_row(Icon::VideoLibrary, "Add to queue", Message::QueueAdd),
            menu_row(
                if saved { Icon::Check } else { Icon::Schedule },
                if saved {
                    "Remove from Watch later"
                } else {
                    "Save to Watch later"
                },
                Message::CardSaveLater,
            ),
            menu_row(Icon::Share, "Share", Message::CardCopyLink),
        ];
        if video.channel_id.is_some() {
            items.push(menu_row(
                Icon::VideoLibrary,
                "Go to channel",
                Message::CardOpenChannel,
            ));
        }
        if !matches!(
            app.top().map(|p| &p.body),
            Some(Body::History(_) | Body::Later(_))
        ) {
            items.push(menu_row(
                Icon::Close,
                "Not interested",
                Message::CardNotInterested,
            ));
        }
        if video.channel_id.is_some()
            && !matches!(
                app.top().map(|p| &p.body),
                Some(Body::Channel(_) | Body::History(_) | Body::Later(_))
            )
        {
            items.push(menu_row(
                Icon::Close,
                "Don’t recommend channel",
                Message::CardHideChannel,
            ));
        }
        (at, context, items)
    } else {
        return Space::new().into();
    };
    let (mw, mh) = (310.0, items.len() as f32 * 36.0 + 16.0);
    let menu = container(column(items))
        .style(t::menu)
        .padding([8, 0])
        .width(mw);
    let (w, h) = (app.win.width, app.win.height);
    let preferred_x = if context { at.x } else { at.x - mw };
    let preferred_y = if context { at.y } else { at.y + 8.0 };
    let x = if preferred_x + mw > w - 8.0 {
        at.x - mw
    } else {
        preferred_x
    };
    let y = if preferred_y + mh > h - 8.0 {
        at.y - mh
    } else {
        preferred_y
    };
    let menu = pin(iced::widget::opaque(menu))
        .x(x.clamp(8.0, (w - mw - 8.0).max(8.0)))
        .y(y.clamp(8.0, (h - mh - 8.0).max(8.0)));
    stack![dismiss_layer(Message::ContextClose, false), menu]
        .width(Fill)
        .height(Fill)
        .into()
}

fn dismiss_layer(message: Message, dim: bool) -> El<'static> {
    let background =
        container(Space::new().width(Fill).height(Fill)).style(move |_: &Theme| container::Style {
            background: dim.then_some(Color::from_rgba8(0, 0, 0, 0.56).into()),
            ..Default::default()
        });
    Control::new(
        mouse_area(background)
            .on_press(message.clone())
            .on_right_press(message.clone())
            .on_middle_press(message.clone())
            .on_scroll(move |_| message.clone()),
    )
    .cover()
    .element()
}

fn shortcuts_layer(app: &App) -> El<'_> {
    if !app.shortcuts {
        return Space::new().into();
    }
    let definitions = [
        ("Search", "/"),
        ("Back / Forward", "Alt + ← / Alt + →"),
        ("Move keyboard focus", "Tab / Shift + Tab"),
        ("Play / Pause", "K / Space"),
        ("Seek backward / forward", "J / L or ← / →"),
        ("Jump to 0–90%", "0–9"),
        ("Volume", "↑ / ↓"),
        ("Mute", "M"),
        ("Theatre / Fullscreen", "T / F"),
        ("Close menu or exit fullscreen", "Esc"),
    ];
    let close = action(
        button(icon(Icon::Close, 24.0, t::TEXT))
            .padding(8)
            .style(t::icon_button)
            .on_press(Message::Shortcuts(false)),
        "menu_item",
        "Close",
        Message::Shortcuts(false),
    );
    let header = row![
        txt("Keyboard shortcuts", t::HEADING),
        Space::new().width(Fill),
        close
    ]
    .align_y(Alignment::Center);
    let rows = column(definitions.into_iter().map(|(label, key)| {
        row![
            txt(label, t::BODY),
            Space::new().width(Fill),
            txt(key, t::BODY).color(t::TEXT_2)
        ]
        .height(30)
        .align_y(Alignment::Center)
        .into()
    }));
    let rows = Control::new(iced::widget::scrollable(rows))
        .scroll_scope()
        .element();
    let panel = container(column![header, rows].spacing(12))
        .style(t::menu)
        .padding(20)
        .width(440)
        .max_height((app.win.height - 32.0).max(100.0));
    stack![
        dismiss_layer(Message::Shortcuts(false), true),
        container(iced::widget::opaque(panel)).center(Fill)
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

fn guide_layer(app: &App) -> El<'_> {
    if !app.guide_is_overlay() {
        return Space::new().into();
    }
    let panel = column![
        row![
            icon_button(Icon::Menu, 40.0, Some(Message::Guide)),
            txt("NativeTube", t::HEADING)
        ]
        .spacing(8)
        .padding([8, 16])
        .align_y(Alignment::Center),
        rail(app, true),
    ];
    stack![
        dismiss_layer(Message::Guide, true),
        iced::widget::opaque(container(panel).width(240).height(Fill).style(t::surface)),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

fn queue_layer(app: &App) -> El<'_> {
    if !app.queue_open {
        return Space::new().into();
    }
    let close = action(
        button(icon(Icon::Close, 22.0, t::TEXT))
            .padding(8)
            .style(t::icon_button)
            .on_press(Message::Queue(false)),
        "menu_item",
        "Close queue",
        Message::Queue(false),
    );
    let heading =
        row![txt("Queue", t::HEADING), Space::new().width(Fill), close].align_y(Alignment::Center);
    let items: El<'_> = if app.queue.is_empty() {
        txt("Add a video from its menu.", t::BODY)
            .color(t::TEXT_2)
            .into()
    } else {
        Control::new(
            iced::widget::scrollable(column(app.queue.iter().map(|video| {
                let play = Message::QueuePlay(video.id.clone());
                let remove = Message::QueueRemove(video.id.clone());
                row![
                    action(
                        button(clamp(video.title.as_str(), t::BODY, 2))
                            .width(Fill)
                            .height(52)
                            .padding(8)
                            .style(t::menu_row(false))
                            .on_press(play.clone()),
                        "menu_item",
                        video.title.as_str(),
                        play
                    ),
                    action(
                        button(icon(Icon::Close, 20.0, t::TEXT))
                            .padding(8)
                            .style(t::icon_button)
                            .on_press(remove.clone()),
                        "menu_item",
                        "Remove from queue",
                        remove
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            })))
            .height(Fill),
        )
        .scroll_scope()
        .element()
    };
    let panel = container(column![heading, items].spacing(12))
        .style(t::menu)
        .padding(20)
        .width(440)
        .height((app.win.height - 32.0).min(500.0));
    stack![
        dismiss_layer(Message::Queue(false), true),
        container(iced::widget::opaque(panel)).center(Fill)
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

fn rail(app: &App, expanded: bool) -> El<'_> {
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
            "Watch later",
        ),
    ];
    let mut col = column![].spacing(if expanded { 4 } else { 0 });
    for (idx, (r, outline, filled, label)) in entries.into_iter().enumerate() {
        if expanded && idx == 2 {
            col = col
                .push(iced::widget::rule::horizontal(1))
                .push(container(txt("You", t::BODY_M)).padding([8, 12]));
        }
        let selected = shown == Some(r);
        let img = icon(if selected { filled } else { outline }, 24.0, t::TEXT);
        let content: El<'_> = if expanded {
            row![img, txt(label, t::BODY)]
                .spacing(24)
                .align_y(Alignment::Center)
                .into()
        } else {
            column![
                img,
                txt(label, t::RAIL_LABEL)
                    .wrapping(Wrapping::None)
                    .align_x(Alignment::Center)
                    .width(Fill)
            ]
            .align_x(Alignment::Center)
            .spacing(6)
            .into()
        };
        let control = button(content)
            .width(if expanded { 216 } else { 64 })
            .height(if expanded { 40 } else { 74 })
            .padding(if expanded {
                Padding::from([8, 12])
            } else {
                Padding {
                    top: 16.0,
                    ..Padding::ZERO
                }
            })
            .style(t::menu_row(selected))
            .on_press(Message::Nav(r));
        col = col.push(action(control, "rail", label, Message::Nav(r)));
    }
    container(col)
        .width(if expanded { 240.0 } else { RAIL })
        .height(Fill)
        .padding(if expanded { 12 } else { 4 })
        .into()
}

// ---------------------------------------------------------------------------------------------

fn page(app: &App) -> El<'_> {
    let Some(p) = app.top() else {
        return Space::new().into();
    };
    let model = if app.fullscreen {
        &app.fullscreen_scroll
    } else {
        &p.scroll
    };
    let rail_w = rail_width(app);
    let body_w = (app.win.width - rail_w).max(100.0);
    let top = if app.fullscreen { 0.0 } else { TOP_BAR };
    let chips = matches!(p.body, Body::Home(_));
    let vh = (app.win.height - top - if chips { t::CHIP_BAR } else { 0.0 }).max(100.0);
    let g = Geo {
        w: app.win.width,
        h: app.win.height,
        body_w,
        vh,
        offset: model.mark_built(),
        pad: if app.win.width < 600.0 { 16.0 } else { 24.0 },
    };
    let mut on_key: Option<crate::scroller::KeyHandler<'_, Message>> = None;
    let lanes = match &p.body {
        Body::Home(h) => {
            let mut l = LaneB::new(model, 0, g.pad, g.body_w - 2.0 * g.pad, &g, 12.0);
            if let Some(last) = &app.resume_offer {
                l.item(K_HEAD, 5, 68.0, || {
                    let title = if last.title.is_empty() {
                        "Last video"
                    } else {
                        &last.title
                    };
                    let label = row![
                        icon(Icon::PlayArrow, 20.0, t::TEXT),
                        txt("Continue watching", t::BODY_M),
                        clamp(title, t::BODY, 1).color(t::TEXT_2),
                        txt(nt_core::format::clock(last.position), t::META_S).color(t::TEXT_2),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center);
                    let control = button(label)
                        .width(Fill)
                        .height(48)
                        .padding([8, 14])
                        .style(t::pill)
                        .on_press(Message::ResumeLast);
                    column![
                        action(control, "resume", title, Message::ResumeLast),
                        Space::new().height(20)
                    ]
                    .into()
                });
            }
            feed_grid(app, &mut l, &h.load, &h.feed, &g, true, None);
            vec![l.finish()]
        }
        Body::Search(s) => {
            let (x, w) = centered(&g, 1096.0);
            let mut l = LaneB::new(model, 0, x, w, &g, 16.0);
            if !s.chips.is_empty() || !s.filters.is_empty() {
                l.item(K_HEAD, 3, 56.0, || search_chips(s));
            }
            let previous =
                !s.feed.items.is_empty() || !s.moments.items.is_empty() || !s.entries.is_empty();
            match &s.load {
                Load::Ready => search_body(app, &mut l, s, w, &g),
                Load::Loading if previous => {
                    l.item(K_STATUS, 3, 40.0, || loading_row(40.0));
                    search_body(app, &mut l, s, w, &g);
                }
                Load::Loading => skeleton_list(&mut l, w),
                Load::Failed(e) if previous => {
                    l.item(K_STATUS, 3, 84.0, || {
                        retry_row("Couldn't load filtered results", e, Message::Retry)
                    });
                    search_body(app, &mut l, s, w, &g);
                }
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
                list_rows(app, &mut l, items, ListRef::Main, w, None);
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
        .keys_enabled(
            !app.search_focused
                && !app.shortcuts
                && !app.search_filters
                && !app.queue_open
                && app.card_menu.is_none()
                && app.player_context.is_none()
                && !app.guide_is_overlay(),
        )
        .scale(app.scale)
        .bottom(if app.fullscreen { 0.0 } else { 24.0 });
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

fn search_chips(search: &Search) -> El<'_> {
    let chips = search
        .chips
        .iter()
        .map(|option| search_option(search, option));
    let filters = (!search.filters.is_empty()).then(|| {
        action(
            button(
                row![icon(Icon::Tune, 18.0, t::TEXT), txt("Filters", t::BODY_M)]
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .height(32)
            .padding([4, 12])
            .style(t::chip(false))
            .on_press(Message::SearchFilters(true)),
            "filters",
            "Filters",
            Message::SearchFilters(true),
        )
    });
    let controls = row(filters.into_iter().chain(chips))
        .spacing(8)
        .align_y(Alignment::Center);
    container(iced::widget::scrollable(controls).direction(
        iced::widget::scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::hidden(),
        ),
    ))
    .height(56)
    .align_y(Alignment::Center)
    .into()
}

fn search_option<'a>(search: &Search, option: &'a nt_core::SearchOption) -> El<'a> {
    let selected = (option.selected && search.filter != nt_core::SearchFilter::Transcripts)
        || option.filter.as_ref() == Some(&search.filter);
    let msg = option.filter.clone().map(Message::ApplySearchFilter);
    let control = button(txt(option.label.as_str(), t::BODY_M))
        .height(32)
        .padding([4, 12])
        .style(t::chip(selected))
        .on_press_maybe(msg.clone());
    match msg {
        Some(msg) => action(control, "chip", option.label.as_str(), msg),
        None => control.into(),
    }
}

fn search_filters_layer(app: &App) -> El<'_> {
    let Some(Body::Search(s)) = app.top().map(|p| &p.body).filter(|_| app.search_filters) else {
        return Space::new().into();
    };
    let close = action(
        button(icon(Icon::Close, 24.0, t::TEXT))
            .padding(8)
            .style(t::icon_button)
            .on_press(Message::SearchFilters(false)),
        "menu_item",
        "Close filters",
        Message::SearchFilters(false),
    );
    let header = row![
        txt("Search filters", t::HEADING),
        Space::new().width(Fill),
        close
    ]
    .align_y(Alignment::Center);
    let group_list = column(s.filters.iter().map(|group| {
        let options = row(group.options.iter().map(|option| search_option(s, option)))
            .spacing(6)
            .wrap()
            .vertical_spacing(6);
        column![txt(group.title.as_str(), t::BODY_M), options]
            .spacing(8)
            .into()
    }))
    .spacing(16);
    let list = Control::new(iced::widget::scrollable(group_list).height(Fill))
        .scroll_scope()
        .element();
    let panel = container(column![header, list].spacing(12))
        .style(t::menu)
        .padding(20)
        .width((app.win.width - 32.0).min(660.0))
        .height((app.win.height - 32.0).min(500.0));
    stack![
        dismiss_layer(Message::SearchFilters(false), true),
        container(iced::widget::opaque(panel)).center(Fill)
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// A search page: YouTube's notices, the video results, then the transcript matches. Never
/// blank: with nothing at all it says so in YouTube's words.
fn search_body<'a>(app: &'a App, l: &mut LaneB<'a>, s: &'a Search, w: f32, g: &Geo) {
    let nothing = s.feed.items.is_empty() && s.entries.is_empty() && s.moments.items.is_empty();
    let mut no_results = nothing;
    for (i, n) in s.notices.iter().enumerate() {
        match n {
            SearchNotice::NoResults => no_results = true,
            SearchNotice::AgeGated { title, body } => {
                l.item(K_NOTICE, i, 120.0, move || {
                    container(
                        column![
                            txt(title.as_str(), t::CARD_TITLE).color(t::TEXT),
                            txt(body.as_str(), t::BODY).color(t::TEXT_2),
                        ]
                        .spacing(8),
                    )
                    .width(Fill)
                    .padding(16)
                    .style(t::fill_box(t::R_THUMB))
                    .into()
                });
                l.item(K_NOTICE, s.notices.len() + i, 16.0, || {
                    Space::new().height(16).into()
                });
            }
            SearchNotice::Corrected { showing, original } => {
                l.item(K_NOTICE, i, 64.0, move || {
                    column![
                        rich(
                            vec![
                                seg("Showing results for "),
                                seg(showing.as_str()).font(t::BOLD).color(t::TEXT),
                            ],
                            t::SEARCH_TITLE,
                            1
                        )
                        .color(t::TEXT),
                        row![
                            txt("Search instead for ", t::BODY).color(t::TEXT_2),
                            search_link(original.as_str()),
                        ],
                    ]
                    .spacing(4)
                    .height(64)
                    .into()
                });
            }
            SearchNotice::DidYouMean { suggestion } => {
                l.item(K_NOTICE, i, 40.0, move || {
                    row![
                        txt("Did you mean: ", t::SEARCH_TITLE).color(t::TEXT),
                        search_link(suggestion.as_str()),
                    ]
                    .align_y(Alignment::Center)
                    .height(40)
                    .into()
                });
            }
        }
    }
    let terms: Vec<String> = s
        .query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .collect();
    if !s.entries.is_empty() {
        mixed_results(app, l, s, w, &terms);
        trailer(l, &s.feed, w, false);
    } else if !s.feed.items.is_empty() {
        list_rows(app, l, &s.feed.items, ListRef::Main, w, Some(terms));
        trailer(l, &s.feed, w, false);
    } else if no_results && s.moments.items.is_empty() {
        let h = (g.vh - 40.0 - l.y).max(300.0);
        l.item(K_STATUS, 0, h, move || {
            empty_state(
                Icon::Search,
                "No results found",
                "Try different keywords or remove search filters".to_string(),
                None,
                h,
            )
        });
    }
    if !s.moments.items.is_empty() {
        l.item(K_HEAD, 1, 52.0, || {
            section("Matches in video transcripts \u{b7} search.pronsh.dev")
        });
        list_rows(app, l, &s.moments.items, ListRef::Moments, w, None);
        trailer(l, &s.moments, w, false);
    }
}

fn mixed_results<'a>(
    app: &'a App,
    l: &mut LaneB<'a>,
    search: &'a Search,
    width: f32,
    terms: &[String],
) {
    let (tw, th) = list_geom(width);
    for (index, entry) in search.entries.iter().enumerate() {
        match entry {
            SearchEntry::Video(i) => {
                if let Some(video) = search.feed.items.get(*i) {
                    l.item(K_MIXED, index, th + 16.0, || {
                        column![
                            list_row(app, video, *i, ListRef::Main, tw, th, Some(terms)),
                            Space::new().height(16)
                        ]
                        .height(th + 16.0)
                        .into()
                    });
                }
            }
            SearchEntry::Channel(channel) => l.item(K_MIXED, index, 146.0, || {
                let record = &channel.channel;
                let img = thumb(
                    record
                        .avatar
                        .as_ref()
                        .and_then(|u| app.images.get(u, 104.0 * app.scale)),
                    104.0,
                    104.0,
                    52.0,
                );
                let label = row![txt(record.name.as_str(), t::SEARCH_TITLE)]
                    .spacing(6)
                    .align_y(Alignment::Center);
                let label = if channel.verified {
                    label.push(icon(Icon::Check, 16.0, t::TEXT_2))
                } else {
                    label
                };
                let meta = [channel.handle.as_deref(), record.subscribers.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let msg = Message::OpenChannel(record.clone());
                let control = button(
                    row![
                        container(img).width(tw).center_x(tw),
                        column![label, txt(meta, t::BODY).color(t::TEXT_2)].spacing(8),
                    ]
                    .spacing(16)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .height(130)
                .padding(0)
                .style(t::card)
                .on_press(msg.clone());
                column![
                    action(control, "channel", record.name.as_str(), msg),
                    Space::new().height(16)
                ]
                .into()
            }),
            SearchEntry::Playlist(playlist) => l.item(K_MIXED, index, th + 16.0, || {
                column![
                    playlist_card(app, playlist, tw, th),
                    Space::new().height(16)
                ]
                .into()
            }),
            SearchEntry::Shelf(title) => l.item(K_MIXED, index, 52.0, || section(title)),
        }
    }
}

fn search_link(q: &str) -> El<'_> {
    button(
        txt(q, t::SEARCH_TITLE)
            .font(iced::Font {
                style: iced::font::Style::Italic,
                ..t::MEDIUM
            })
            .color(t::LINK),
    )
    .padding(0)
    .style(t::bare)
    .on_press(Message::DescLink(crate::app::Link::Search(q.to_string())))
    .into()
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

fn trailer<'a>(l: &mut LaneB<'a>, feed: &'a Feed, width: f32, grid: bool) {
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
        Some(More::Failed(e)) => l.item(K_TRAIL, 1, 84.0, || {
            retry_row("Couldn't load more", e, Message::RetryMore)
        }),
        _ => {}
    }
}

fn chips_row(h: &crate::app::Home) -> El<'_> {
    let labels: Vec<_> = if h.chips.is_empty() {
        TOPICS.iter().map(|topic| topic.title).collect()
    } else {
        h.chips.iter().map(|chip| chip.label.as_str()).collect()
    };
    let chips = labels
        .into_iter()
        .enumerate()
        .map(|(i, title)| {
            let msg = Message::Topic(i);
            action(
                button(txt(title, t::BODY_M))
                    .padding([6, 12])
                    .height(32)
                    .style(t::chip(i == h.topic))
                    .on_press(msg.clone()),
                "chip",
                title,
                msg,
            )
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
    let mut text = row![
        clamp(v.channel.as_str(), style, 1)
            .color(t::TEXT_2)
            .shrink()
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if v.channel_verified {
        text = text.push(icon(Icon::Check, 14.0, t::TEXT_2));
    }
    if v.channel_id.is_some() && !v.channel.is_empty() {
        let msg = Message::OpenChannelOf(list, i);
        action(
            button(text).padding(0).style(t::link).on_press(msg.clone()),
            "channel",
            v.channel.as_str(),
            msg,
        )
    } else {
        text.into()
    }
}

fn card_meta(video: &Video) -> String {
    video
        .upcoming
        .as_ref()
        .cloned()
        .unwrap_or_else(|| video.meta_line())
}

fn badges<'a>(video: &'a Video) -> impl Iterator<Item = El<'a>> {
    video
        .badges
        .iter()
        .filter(|s| !s.eq_ignore_ascii_case("live"))
        .map(|s| {
            container(txt(s.as_str(), t::META_S).color(t::TEXT_2))
                .style(t::fill_box(3.0))
                .padding([0, 4])
                .into()
        })
}

/// Right-click anywhere on a card or row opens its menu at the pointer (captured, so nothing
/// under the pointer reacts).
fn card_interaction<'a>(
    app: &'a App,
    base: impl Into<El<'a>>,
    menu: impl Into<El<'a>>,
    address: (ListRef, usize),
    video: &'a Video,
    kind: &'static str,
    dimensions: (f32, f32),
) -> El<'a> {
    let (list, index) = address;
    let hash = id_hash(&video.id);
    let base = Control::new(base)
        .target(kind, video.title.as_str())
        .action(Message::Open(list, index, hash))
        .tint_on_hover()
        .preload_on_hover(
            app.can_preload()
                .then_some(Message::Preload(list, index, hash)),
        )
        .preview_on_hover(crate::app::PreviewTarget {
            page: app.top_id(),
            list,
            index,
            hash,
            width: (dimensions.0 * app.scale).ceil() as u32,
            height: (dimensions.1 * app.scale).ceil() as u32,
        })
        .element();
    mouse_area(hover(base, menu))
        .on_right_press(Message::CardContext(list, index))
        .into()
}

fn more_button<'a>(list: ListRef, i: usize) -> El<'a> {
    let message = Message::CardMenu(list, i);
    action(
        button(container(icon(Icon::MoreVert, 24.0, t::TEXT)).center(Fill))
            .width(40)
            .height(40)
            .padding(0)
            .style(t::icon_button)
            .on_press(message.clone()),
        "more",
        "More actions",
        message,
    )
}

fn preview_thumbnail<'a>(
    app: &'a App,
    poster: impl Into<El<'a>>,
    video: &Video,
    list: ListRef,
    index: usize,
    size: (f32, f32),
    radius: f32,
) -> El<'a> {
    let Some(frame) = app.preview_frame.as_ref().filter(|_| {
        app.preview.is_some_and(|target| {
            target.page == app.top_id()
                && target.list == list
                && target.index == index
                && target.hash == id_hash(&video.id)
        })
    }) else {
        return poster.into();
    };
    let poster: El<'_> = poster.into();
    stack![
        poster,
        shader(crate::video::VideoProgram::<true> {
            frame: Some(frame.clone()),
            radius: radius * app.scale,
            first_frame: None,
            playback_mark: None,
        })
        .width(size.0)
        .height(size.1),
    ]
    .width(size.0)
    .height(size.1)
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
    let img = preview_thumbnail(app, img, v, list, i, (w, th), t::R_THUMB);
    let img = press_hint(img, Message::Preload(list, i, id_hash(&v.id)));
    let mut text_col = column![clamp(v.title.as_str(), t::CARD_TITLE, 2).color(t::TEXT)]
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
    text_col = text_col.push(
        row![clamp(card_meta(v), t::BODY, 1).color(t::TEXT_2).shrink()]
            .extend(badges(v))
            .spacing(4)
            .align_y(Alignment::Center),
    );
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
    card_interaction(app, base, menu, (list, i), v, "card", (w, th))
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
    list: ListRef,
    content_w: f32,
    terms: Option<Vec<String>>,
) {
    let (tw, th) = list_geom(content_w);
    let kind = if list == ListRef::Moments {
        K_MOMENT
    } else {
        K_ROW
    };
    l.rows(kind, items.len(), th + 16.0, |i| match items.get(i) {
        Some(v) => list_row(app, v, i, list, tw, th, terms.as_deref()),
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
    let img = preview_thumbnail(app, img, v, list, i, (tw, th), t::R_THUMB);
    let img = press_hint(img, Message::Preload(list, i, id_hash(&v.id)));
    let mut chan = row![].spacing(8).align_y(Alignment::Center);
    if let Some(a) = avatar(app, v, i, list, 24.0) {
        chan = chan.push(a);
    }
    chan = chan.push(channel_name(v, i, list, t::META_S));
    let mut col = column![
        clamp(v.title.as_str(), t::SEARCH_TITLE, 2).color(t::TEXT),
        clamp(card_meta(v), t::META_S, 1).color(t::TEXT_2),
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
    if v.badges
        .iter()
        .any(|badge| !badge.eq_ignore_ascii_case("live"))
    {
        col = col.push(row(badges(v)).spacing(4));
    }
    if let Some(m) = &v.moment {
        // "▶ 9:52" and the matching transcript line, its matched words bold and bright.
        let chip = container(
            row![
                icon(Icon::PlayArrow, 16.0, t::TEXT),
                txt(m.label.as_str(), t::META_S_M).color(t::TEXT)
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            top: 1.0,
            bottom: 1.0,
            left: 4.0,
            right: 6.0,
        })
        .style(t::fill_box(4.0));
        col = col.push(
            row![chip, rich(moment_segs(m), t::META_S, 2).color(t::TEXT_2)]
                .spacing(8)
                .align_y(Alignment::Start),
        );
    } else if let Some(s) = v.snippet.as_deref().filter(|s| !s.is_empty()) {
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
    card_interaction(app, base, menu, (list, i), v, "row", (tw, th))
}

/// A transcript snippet split at its highlight ranges (UTF-8 byte offsets).
fn moment_segs(m: &nt_core::Moment) -> Vec<Seg<'_>> {
    let text = m.snippet.as_str();
    let mut ranges: Vec<std::ops::Range<usize>> = m
        .highlights
        .iter()
        .filter(|r| {
            r.start < r.end
                && r.end <= text.len()
                && text.is_char_boundary(r.start)
                && text.is_char_boundary(r.end)
        })
        .cloned()
        .collect();
    ranges.sort_by_key(|r| r.start);
    let mut segs = Vec::new();
    let mut at = 0;
    for r in ranges {
        if r.start < at {
            continue;
        }
        if let Some(plain) = text.get(at..r.start).filter(|p| !p.is_empty()) {
            segs.push(seg(plain));
        }
        if let Some(hit) = text.get(r.clone()) {
            segs.push(seg(hit).font(t::BOLD).color(t::TEXT));
        }
        at = r.end;
    }
    if let Some(rest) = text.get(at..).filter(|p| !p.is_empty()) {
        segs.push(seg(rest));
    }
    segs
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
    let img = preview_thumbnail(
        app,
        img,
        v,
        ListRef::Related,
        i,
        (168.0, 94.0),
        t::R_COMPACT,
    );
    let col = column![
        clamp(v.title.as_str(), t::BODY_M, 2).color(t::TEXT),
        Space::new().height(4),
        channel_name(v, i, ListRef::Related, t::META_S),
        clamp(card_meta(v), t::META_S, 1).color(t::TEXT_2),
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
    card_interaction(
        app,
        base,
        column![menu],
        (ListRef::Related, i),
        v,
        "related",
        (168.0, 94.0),
    )
}

pub fn subscribe_button<'a>(subscribed: bool) -> El<'a> {
    let control = if subscribed {
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
    };
    action(
        control,
        "subscribe",
        if subscribed {
            "Subscribed"
        } else {
            "Subscribe"
        },
        Message::ToggleSubscribe,
    )
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
    l.item(K_TABS, 0, 73.0, || channel_tabs(c));
    if c.tab == nt_core::ChannelTab::Videos
        && let Some(info) = &c.info
        && !info.video_sort.is_empty()
    {
        l.item(K_TABS, 1, 48.0, || {
            let choices = info.video_sort.iter().enumerate().map(|(i, chip)| {
                let msg = Message::ChannelSort(i);
                let b = button(txt(chip.label.as_str(), t::BODY_M))
                    .padding([6, 12])
                    .height(32)
                    .style(t::chip(c.sort == Some(i)))
                    .on_press_maybe((chip.next.is_some() || chip.selected).then(|| msg.clone()));
                if chip.next.is_some() || chip.selected {
                    action(b, "chip", chip.label.as_str(), msg)
                } else {
                    b.into()
                }
            });
            container(row(choices).spacing(8)).height(48).into()
        });
    }
    if c.tab == nt_core::ChannelTab::Playlists && c.load == Load::Ready {
        if let Some(info) = &c.info {
            let (cols, card_width) = grid_geom(g.body_w, w);
            let height = (card_width * 9.0 / 16.0).round();
            if info.playlists.items.is_empty() {
                l.item(K_STATUS, 0, 76.0, || section("No playlists"));
            }
            l.rows(
                K_PLAYLIST,
                info.playlists.items.len().div_ceil(cols),
                height + 68.0,
                |i| {
                    row(info
                        .playlists
                        .items
                        .iter()
                        .skip(i * cols)
                        .take(cols)
                        .map(|list| playlist_card(app, list, card_width, height)))
                    .spacing(16)
                    .height(height + 68.0)
                    .into()
                },
            );
        }
        match &c.playlists_more {
            More::Loading => l.item(K_TRAIL, 0, 56.0, || loading_row(56.0)),
            More::Failed(e) => l.item(K_TRAIL, 1, 76.0, || {
                retry_row("Couldn't load more playlists", e, Message::RetryMore)
            }),
            More::Idle => {}
        }
    } else {
        feed_grid(
            app,
            &mut l,
            &c.load,
            &c.feed,
            g,
            false,
            Some("This channel has no videos"),
        );
    }
    vec![l.finish()]
}

fn channel_tabs(channel: &Channel) -> El<'_> {
    use nt_core::ChannelTab;
    let fallback = [ChannelTab::Videos];
    let tabs = channel
        .info
        .as_ref()
        .map(|i| i.tabs.as_slice())
        .filter(|tabs| !tabs.is_empty())
        .unwrap_or(&fallback);
    let buttons = tabs.iter().copied().map(|tab| {
        let title = match tab {
            ChannelTab::Home => "Home",
            ChannelTab::Videos => "Videos",
            ChannelTab::Live => "Live",
            ChannelTab::Playlists => "Playlists",
        };
        let active = tab == channel.tab;
        let control = button(txt(title, t::TAB))
            .height(46)
            .padding([6, 12])
            .style(t::menu_row(active))
            .on_press(Message::ChannelTab(tab));
        let tab_button = action(control, "tab", title, Message::ChannelTab(tab));
        column![
            tab_button,
            container(Space::new())
                .height(2)
                .width(Fill)
                .style(move |theme| {
                    if active {
                        t::underline(theme)
                    } else {
                        container::Style::default()
                    }
                })
        ]
        .width(Length::Shrink)
        .into()
    });
    column![
        row(buttons).spacing(12),
        iced::widget::rule::horizontal(1),
        Space::new().height(20)
    ]
    .into()
}

fn playlist_card<'a>(
    app: &'a App,
    list: &'a nt_core::PlaylistRef,
    width: f32,
    height: f32,
) -> El<'a> {
    let count = list
        .video_count
        .as_deref()
        .map(|n| {
            if n.to_ascii_lowercase().contains("video") {
                n.to_string()
            } else {
                format!("{n} videos")
            }
        })
        .unwrap_or_else(|| "Playlist".into());
    let cover = thumb(
        list.thumbnail
            .as_ref()
            .and_then(|u| app.images.get(u, width * app.scale)),
        width,
        height,
        t::R_THUMB,
    )
    .cover()
    .badge(Some(count), false);
    let msg = Message::OpenPlaylist(list.id.clone());
    let control = button(
        column![
            cover,
            clamp(list.title.as_str(), t::CARD_TITLE, 2).color(t::TEXT)
        ]
        .width(width)
        .spacing(8),
    )
    .padding(0)
    .style(t::card)
    .on_press(msg.clone());
    action(control, "playlist", list.title.as_str(), msg)
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
    let mut name = row![txt(name, t::CHANNEL_NAME)]
        .spacing(8)
        .align_y(Alignment::Center);
    if c.info.as_ref().is_some_and(|i| i.verified) {
        name = name.push(icon(Icon::Check, 20.0, t::TEXT_2));
    }
    let mut col = column![name].spacing(8).width(Fill);
    if !line.is_empty() {
        col = col.push(clamp(line, t::BODY, 1).color(t::TEXT_2));
    }
    if c.desc_open {
        let full = c
            .about
            .as_ref()
            .map(|a| a.description.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(desc);
        if !full.is_empty() {
            col = col.push(clamp(full, t::BODY, 0).color(t::TEXT_2));
        }
        let links = c
            .about
            .as_ref()
            .map(|a| a.links.as_slice())
            .or_else(|| c.info.as_ref().map(|i| i.links.as_slice()));
        for link in links.into_iter().flatten() {
            let copy = Message::DescLink(crate::app::Link::Url(link.url.clone()));
            col = col.push(action(
                button(txt(link.label.as_str(), t::BODY).color(t::LINK))
                    .padding(0)
                    .style(t::bare)
                    .on_press(copy.clone()),
                "link",
                link.label.as_str(),
                copy,
            ));
        }
        match &c.about_load {
            Load::Loading => col = col.push(loading_row(40.0)),
            Load::Failed(e) => {
                col = col.push(retry_row(
                    "Couldn't load channel details",
                    e,
                    Message::ChannelAbout(true),
                ))
            }
            Load::Ready => {}
        }
    } else if !desc.is_empty() {
        col = col.push(clamp(desc, t::BODY, 1).color(t::TEXT_2));
    }
    if !desc.is_empty()
        || c.info
            .as_ref()
            .is_some_and(|i| i.about.is_some() || !i.links.is_empty())
    {
        let label = if c.desc_open {
            "Show less"
        } else {
            c.info
                .as_ref()
                .and_then(|i| i.links_label.as_deref())
                .unwrap_or("About")
        };
        let msg = Message::ChannelAbout(!c.desc_open);
        col = col.push(action(
            button(txt(label, t::BODY_M).color(t::LINK))
                .padding(4)
                .style(t::bare)
                .on_press(msg.clone()),
            "more",
            label,
            msg,
        ));
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
    if wg.secondary.is_none() {
        watch_playlist(app, &mut l, w);
    }
    match &w.load {
        Load::Ready => {
            if let Some(info) = &w.info {
                w.desc_y.set(l.y);
                l.item(K_DESC, 0, 150.0, || player_view::description(app, info, w));
            }
            if wg.secondary.is_none() {
                l.item(K_RHEAD, 0, 24.0, || Space::new().height(24).into());
                related_videos(app, &mut l, w);
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
        watch_playlist(app, &mut r, w);
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
            _ => related_videos(app, &mut r, w),
        }
        lanes.push(r.finish());
    }
    lanes
}

fn watch_playlist<'a>(app: &'a App, lane: &mut LaneB<'a>, watch: &'a Watch) {
    let Some(list) = &watch.playlist else { return };
    lane.item(K_RHEAD, 3, 56.0, || {
        let name = if list.is_mix {
            format!("Mix · {}", list.title)
        } else {
            list.title.clone()
        };
        container(clamp(name, t::HEADING, 1).color(t::TEXT))
            .height(56)
            .align_y(Alignment::Center)
            .into()
    });
    lane.rows(K_PLAYLIST, list.items.len(), 70.0, |index| {
        let video = &list.items[index];
        let img = thumb(
            app.images.get(&video.thumbnail, 112.0 * app.scale),
            112.0,
            63.0,
            t::R_COMPACT,
        )
        .cover();
        let content = row![
            img,
            column![
                clamp(video.title.as_str(), t::BODY_M, 2).color(t::TEXT),
                clamp(video.channel.as_str(), t::META_S, 1).color(t::TEXT_2),
            ]
            .spacing(4)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let msg = Message::PlaylistPlay(index);
        action(
            button(content)
                .width(Fill)
                .height(68)
                .padding(2)
                .style(t::menu_row(list.index == Some(index)))
                .on_press(msg.clone()),
            "playlist_video",
            video.title.as_str(),
            msg,
        )
    });
    watch.playlist_end.set(lane.y);
    match &watch.playlist_more {
        More::Loading => lane.item(K_TRAIL, 8, 56.0, || loading_row(56.0)),
        More::Failed(e) => lane.item(K_TRAIL, 9, 76.0, || {
            retry_row(
                "Couldn't load more playlist videos",
                e,
                Message::PlaylistMore,
            )
        }),
        More::Idle => {}
    }
}

fn related_videos<'a>(app: &'a App, lane: &mut LaneB<'a>, w: &'a Watch) {
    if let Some(info) = &w.info
        && !info.related_chips.is_empty()
        && !w.related.is_empty()
    {
        lane.item(K_RHEAD, 1, t::CHIP_BAR, || {
            let chips = info
                .related_chips
                .iter()
                .enumerate()
                .map(|(i, chip)| {
                    let msg = Message::RelatedChip(i);
                    let control = button(txt(chip.label.as_str(), t::BODY_M))
                        .height(32)
                        .padding([4, 12])
                        .style(t::chip(w.related_selected == Some(i)))
                        .on_press(msg.clone());
                    action(control, "chip", chip.label.as_str(), msg)
                })
                .collect();
            chip_bar(chips, &w.related_chip_offset).into()
        });
    }
    lane.rows(K_REL, w.related.len(), 102.0, |i| {
        compact_row(app, &w.related[i], i)
    });
    w.related_end.set(lane.y);
    match &w.related_more {
        More::Idle => {}
        More::Loading => lane.item(K_TRAIL, 5, 56.0, || loading_row(56.0)),
        More::Failed(error) => lane.item(K_TRAIL, 6, 76.0, || {
            retry_row("Couldn't load more videos", error, Message::RelatedMore)
        }),
    }
}

fn loading_row(height: f32) -> El<'static> {
    container(crate::widgets::spinner(24.0))
        .width(Fill)
        .height(height)
        .center_x(Fill)
        .into()
}

fn retry_row<'a>(label: &'static str, message: &'a str, retry: Message) -> El<'a> {
    let retry_btn = action(
        button(txt("Retry", t::BODY_M))
            .padding([6, 12])
            .style(t::pill)
            .on_press(retry.clone()),
        "retry",
        "Retry",
        retry,
    );
    column![
        row![txt(label, t::BODY_M), retry_btn]
            .spacing(12)
            .align_y(Alignment::Center),
        clamp(message, t::META_S, 1).color(t::TEXT_2),
    ]
    .spacing(4)
    .into()
}

fn comments<'a>(app: &'a App, l: &mut LaneB<'a>, w: &'a Watch, width: f32) {
    w.comments_y.set(l.y);
    // Laid out within reach before any scroll: check again once this frame is presented, as
    // GPUI and Rune do after every layout.
    if w.comments_state == Comments::NotStarted
        && w.load == Load::Ready
        && w.info.as_ref().is_some_and(|i| i.comments.is_some())
        && app.top().is_some_and(|p| {
            nt_core::policy::comments_due(
                f64::from(l.y),
                f64::from(p.scroll.heading()),
                f64::from(p.scroll.viewport().height),
            )
        })
    {
        crate::scroller::recheck_after_frame();
    }
    l.item(K_CHEAD, 0, 76.0, || {
        let label = match &w.comments_total {
            Some(n) => format!("{n} Comments"),
            None => "Comments".to_string(),
        };
        let mut head = row![txt(label, t::HEADING)]
            .spacing(12)
            .align_y(Alignment::Center);
        if !w.comments_sort.is_empty() {
            head = head.push(txt("Sort by", t::BODY_M));
            for (index, sort) in w.comments_sort.iter().enumerate() {
                let message = Message::SortComments(index);
                let button = button(txt(sort.label.as_str(), t::BODY_M))
                    .height(32)
                    .padding([4, 12])
                    .style(t::chip(sort.selected))
                    .on_press(message.clone());
                head = head.push(action(button, "comment_sort", sort.label.as_str(), message));
            }
        }
        container(head)
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
                let h = l.model.height(comment_key(i)).unwrap_or_else(|| {
                    let lines = ((*chars as f32 * 7.4) / text_w).ceil().clamp(1.0, 4.0);
                    let extra = w
                        .replies
                        .get(&i)
                        .filter(|r| r.open)
                        .map_or(0.0, |r| r.items.len() as f32 * 92.0 + 40.0);
                    18.0 + 2.0 + lines * 20.0 + 8.0 + 18.0 + 24.0 + extra
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
        Some(c) => comment(app, w, c, Some(i), text_w),
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
        Comments::Failed(e) => l.item(K_CTRAIL, 1, 76.0, || {
            retry_row("Couldn't load more comments", e, Message::RetryComments)
        }),
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

fn comment<'a>(
    app: &'a App,
    watch: &'a Watch,
    c: &'a Comment,
    index: Option<usize>,
    text_w: f32,
) -> El<'a> {
    let avatar_size = if index.is_some() { 40.0 } else { 24.0 };
    let av = thumb(
        c.author_avatar
            .as_ref()
            .and_then(|u| app.images.get(u, avatar_size * app.scale)),
        avatar_size,
        avatar_size,
        avatar_size / 2.0,
    );
    let mut header = row![txt(c.author.as_str(), t::AUTHOR)]
        .spacing(4)
        .align_y(Alignment::Center);
    if c.author_verified {
        header = header.push(icon(Icon::Check, 14.0, t::TEXT_2));
    }
    if let Some(published) = &c.published {
        header = header.push(txt(published.as_str(), t::META_S).color(t::TEXT_2));
    }
    let open = index.is_some_and(|i| watch.comments_open.contains(&i));
    let mut col = column![].spacing(4).width(Fill);
    if c.pinned {
        col =
            col.push(txt(format!("Pinned by {}", watch.video.channel), t::META_S).color(t::TEXT_2));
    }
    col = col
        .push(header)
        .push(clamp(c.text.as_str(), t::BODY, if open { 0 } else { 4 }).color(t::TEXT));
    let est_lines =
        (c.text.chars().count() as f32 * 7.4 / text_w).ceil() + c.text.matches('\n').count() as f32;
    if let Some(index) = index
        && est_lines > 4.0
    {
        col = col.push(action(
            button(txt(if open { "Show less" } else { "Read more" }, t::BODY_M).color(t::TEXT_2))
                .padding(0)
                .style(t::bare)
                .on_press(Message::ToggleComment(index)),
            "more",
            "Read comment",
            Message::ToggleComment(index),
        ));
    }
    let mut foot = row![icon(Icon::ThumbUp, 16.0, t::TEXT_2)]
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(likes) = &c.likes {
        foot = foot.push(txt(likes.as_str(), t::META_S).color(t::TEXT_2));
    }
    if c.hearted {
        let avatar = watch
            .info
            .as_ref()
            .and_then(|i| i.channel.avatar.as_ref())
            .and_then(|url| app.images.get(url, 16.0 * app.scale));
        foot = foot
            .push(thumb(avatar, 16.0, 16.0, 8.0))
            .push(txt("♥", t::META_S_M).color(t::RED));
    }
    col = col.push(container(foot).padding(Padding {
        top: 4.0,
        ..Padding::ZERO
    }));
    if let Some(index) = index
        && c.replies.is_some()
    {
        let reply = watch.replies.get(&index);
        let expanded = reply.is_some_and(|r| r.open);
        let label = match c.reply_count.as_deref() {
            Some(count) => format!("{count} {}", if count == "1" { "reply" } else { "replies" }),
            None => "Replies".into(),
        };
        let toggle = Message::ReplyToggle(index);
        col = col.push(action(
            button(
                row![
                    icon(
                        if expanded {
                            Icon::ExpandLess
                        } else {
                            Icon::ExpandMore
                        },
                        18.0,
                        t::LINK
                    ),
                    txt(label.clone(), t::BODY_M).color(t::LINK),
                ]
                .align_y(Alignment::Center)
                .spacing(4),
            )
            .padding([6, 8])
            .style(t::bare)
            .on_press(toggle.clone()),
            "comment_replies",
            label,
            toggle,
        ));
        if let Some(reply) = reply.filter(|r| r.open) {
            col = col.extend(
                reply
                    .items
                    .iter()
                    .map(|r| comment(app, watch, r, None, text_w - 40.0)),
            );
            match &reply.more {
                Some(More::Loading) => col = col.push(loading_row(36.0)),
                Some(More::Failed(e)) => {
                    col = col.push(retry_row(
                        "Couldn't load replies",
                        e,
                        Message::ReplyMore(index),
                    ))
                }
                _ if reply.has_more() => {
                    col = col.push(action(
                        button(txt("Show more replies", t::BODY_M).color(t::LINK))
                            .padding([6, 8])
                            .style(t::bare)
                            .on_press(Message::ReplyMore(index)),
                        "comment_replies",
                        "Show more replies",
                        Message::ReplyMore(index),
                    ));
                }
                _ => {}
            }
        }
    }
    container(row![av, col].spacing(16))
        .padding(Padding {
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}
