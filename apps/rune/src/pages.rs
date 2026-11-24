//! Pages on the navigation stack and their layout plans.
//!
//! A plan is the page's content laid out as absolutely placed blocks with known sizes (from the
//! window size, fixed row metrics and `rune::text::measure` for wrapped text). The view builds
//! only the blocks inside the scroller's mounted range, so nothing a frame or a commit does
//! scales with the number of loaded items beyond a binary search. Text is measured at a width
//! rounded down to 16 points, so a live resize re-measures once per 16 points rather than per
//! pixel, and the measured height is never short of what the wider box draws.

use crate::scroller::Scroller;
use crate::theme::*;
use nt_core::{
    ChannelInfo, ChannelRef, Comment, CommentSort, Continuation, SearchNotice, Video, WatchInfo,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Idle,
    Loading,
    Error(String),
}

pub struct Feed {
    pub items: Vec<Video>,
    pub next: Option<Continuation>,
    /// The first page.
    pub status: Load,
    /// The next page (infinite scroll).
    pub more: Load,
    /// Ids already in `items`: continuation pages repeat videos, and a list must not.
    seen: HashSet<String>,
}

impl Default for Feed {
    fn default() -> Self {
        Feed {
            items: Vec::new(),
            next: None,
            status: Load::Loading,
            more: Load::Idle,
            seen: HashSet::new(),
        }
    }
}

impl Feed {
    pub fn set(&mut self, items: Vec<Video>, next: Option<Continuation>) {
        self.items.clear();
        self.seen.clear();
        self.append(items);
        self.next = next;
        self.status = Load::Idle;
        self.more = Load::Idle;
    }

    /// Appends the videos not already listed; returns how many were new.
    pub fn append(&mut self, items: Vec<Video>) -> usize {
        let before = self.items.len();
        for v in items {
            if self.seen.insert(v.id.clone()) {
                self.items.push(v);
            }
        }
        self.items.len() - before
    }

    pub fn reset(&mut self) {
        *self = Feed::default();
    }

    /// An empty list that isn't loading.
    pub fn idle() -> Feed {
        Feed {
            status: Load::Idle,
            ..Feed::default()
        }
    }
}

pub struct Replies {
    pub open: bool,
    pub items: Vec<Comment>,
    pub next: Option<Continuation>,
    pub state: Load,
}

pub struct Watch {
    pub id: String,
    /// What the card said, shown until the watch info arrives.
    pub video: Video,
    pub info: Option<WatchInfo>,
    pub error: Option<String>,
    pub comments: Vec<Comment>,
    pub comments_next: Option<Continuation>,
    pub comments_total: Option<String>,
    pub comments_state: Load,
    pub comments_sort: Vec<CommentSort>,
    pub comment_order: Option<usize>,
    pub pending_order: Option<usize>,
    pub replies: HashMap<String, Replies>,
    pub related_chip: Option<usize>,
    pub pending_related: Option<usize>,
    pub playlist_key: Option<String>,
    pub playlist: Option<nt_core::PlaylistPanel>,
    pub playlist_more: Load,
    pub comments_started: bool,
    pub desc_open: bool,
    /// Comments expanded past four lines ("Read more").
    pub comments_open: HashSet<String>,
    /// The viewer's own like / dislike (local only: the app never signs in).
    pub rating: Option<bool>,
    /// Playback position to restore when the page shows again (Back).
    pub resume_at: Option<f64>,
    pub resume_paused: bool,
    pub resume_duration: f64,
    /// Measured comment text heights: (width bits, full height).
    pub comment_h: RefCell<Vec<(u32, f32)>>,
    /// Where the collapsed description is cut for its inline "…more": (width bits, byte index).
    pub desc_cut: RefCell<Option<(u32, Option<usize>)>>,
}

impl Watch {
    pub fn new(v: Video) -> Watch {
        Watch {
            id: v.id.clone(),
            video: v,
            info: None,
            error: None,
            comments: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: Load::Idle,
            comments_sort: Vec::new(),
            comment_order: None,
            pending_order: None,
            replies: HashMap::new(),
            related_chip: None,
            pending_related: None,
            playlist_key: None,
            playlist: None,
            playlist_more: Load::Idle,
            comments_started: false,
            desc_open: false,
            comments_open: HashSet::new(),
            rating: None,
            resume_at: None,
            resume_paused: false,
            resume_duration: 0.0,
            comment_h: RefCell::default(),
            desc_cut: RefCell::default(),
        }
    }

    /// The channel: the watch info's, with anything it lacks taken from the card.
    pub fn channel(&self) -> ChannelRef {
        let mut c = self
            .info
            .as_ref()
            .map(|i| i.channel.clone())
            .unwrap_or_default();
        if c.id.is_empty() {
            c.id = self.video.channel_id.clone().unwrap_or_default();
        }
        if c.name.is_empty() {
            c.name = self.video.channel.clone();
        }
        if c.avatar.as_deref().unwrap_or("").is_empty() {
            c.avatar = self.video.channel_avatar.clone();
        }
        c
    }

    /// The video as a library entry: the watch info's when loaded, else the card's.
    pub fn as_video(&self) -> Video {
        match &self.info {
            Some(i) => {
                let mut v = i.as_video();
                let c = self.channel();
                v.channel = c.name;
                v.channel_id = Some(c.id).filter(|id| !id.is_empty());
                v.channel_avatar = c.avatar;
                v.duration = self.video.duration.clone();
                v
            }
            None => self.video.clone(),
        }
    }
}

/// A results page: the query, core's notices (age gate, correction, no results), the results
/// (`Page::feed`) and the transcript matches from search.pronsh.dev.
pub struct Search {
    pub query: String,
    pub notices: Vec<SearchNotice>,
    pub trans: Feed,
    pub trans_total: Option<u64>,
    pub filter: nt_core::SearchFilter,
    pub chips: Vec<nt_core::SearchOption>,
    pub filters: Vec<nt_core::SearchFilterGroup>,
    pub entries: Vec<nt_core::SearchResult>,
    seen_entries: HashSet<(char, String)>,
}

impl Search {
    pub fn append_entries(&mut self, entries: Vec<nt_core::SearchResult>, videos: &[Video]) {
        use nt_core::SearchResult::*;
        if entries.is_empty() {
            for v in videos {
                if self.seen_entries.insert(('v', v.id.clone())) {
                    self.entries.push(Video(Box::new(v.clone())));
                }
            }
            return;
        }
        for entry in entries {
            let key = match &entry {
                Video(v) => ('v', v.id.clone()),
                Playlist(p) => ('p', p.id.clone()),
                Channel(c) => ('c', c.channel.id.clone()),
                Shelf(sh) => ('s', sh.title.clone()),
            };
            if self.seen_entries.insert(key) {
                if let Shelf(mut shelf) = entry {
                    let items = std::mem::take(&mut shelf.items);
                    self.entries.push(Shelf(shelf));
                    self.append_entries(items, &[]);
                } else {
                    self.entries.push(entry);
                }
            }
        }
    }

    pub fn new(query: String) -> Search {
        Search {
            query,
            notices: Vec::new(),
            trans: Feed::idle(),
            trans_total: None,
            filter: nt_core::SearchFilter::All,
            chips: Vec::new(),
            filters: Vec::new(),
            entries: Vec::new(),
            seen_entries: HashSet::new(),
        }
    }
}

pub enum Kind {
    Home {
        topic: usize,
    },
    Search(Box<Search>),
    Watch(Box<Watch>),
    Channel {
        id: String,
        info: Option<Box<ChannelInfo>>,
        desc_open: bool,
    },
    Subs {
        channels: Vec<ChannelRef>,
    },
    History,
    Later,
}

pub struct Page {
    pub id: u64,
    pub kind: Kind,
    /// The page's main list: the feed, results, uploads, library rows, or a watch page's related videos.
    pub feed: Feed,
    pub home_chips: Vec<nt_core::Chip>,
    pub home_chip: Option<usize>,
    pub channel_tab: Option<nt_core::ChannelTab>,
    pub channel_sort: Option<usize>,
    pub pending_sort: Option<usize>,
    pub playlist_cards_more: Load,
    pub scroll: Scroller,
    /// Bumped whenever something the plan depends on changes.
    pub version: Cell<u64>,
    /// The request token: bumped by every (re)load of the page's data. An answer carries the
    /// token it was asked with and is dropped unless it still matches.
    pub req: u64,
    playlist_open_seq: u64,
    plan: RefCell<Rc<Plan>>,
}

#[derive(Clone, Copy)]
pub struct PlaylistOpen {
    page: u64,
    revision: u64,
    sequence: u64,
}

#[cfg(test)]
mod playlist_open_tests {
    use super::{Kind, Page};

    #[test]
    fn latest_playlist_click_wins_even_before_its_response_arrives() {
        let mut page = Page::new(Kind::Home { topic: 0 });
        let first = page.begin_playlist_open();
        assert!(page.accepts_playlist_open(first));

        let second = page.begin_playlist_open();
        assert!(!page.accepts_playlist_open(first));
        assert!(page.accepts_playlist_open(second));
        assert!(!Page::new(Kind::Home { topic: 0 }).accepts_playlist_open(second));

        assert_eq!(page.req, 0);
        page.req += 1;
        assert!(!page.accepts_playlist_open(second));
    }
}

impl Page {
    pub fn new(kind: Kind) -> Page {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Page {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            feed: Feed::default(),
            home_chips: Vec::new(),
            home_chip: None,
            channel_tab: None,
            channel_sort: None,
            pending_sort: None,
            playlist_cards_more: Load::Idle,
            scroll: Scroller::default(),
            version: Cell::new(1),
            req: 0,
            playlist_open_seq: 0,
            plan: RefCell::new(Rc::new(Plan::default())),
        }
    }

    pub fn touch(&self) {
        self.version.set(self.version.get() + 1);
    }

    /// Opening a playlist navigates away. Its token is independent of in-page data loads.
    pub fn begin_playlist_open(&mut self) -> PlaylistOpen {
        self.playlist_open_seq += 1;
        PlaylistOpen {
            page: self.id,
            revision: self.req,
            sequence: self.playlist_open_seq,
        }
    }

    pub fn accepts_playlist_open(&self, request: PlaylistOpen) -> bool {
        request.page == self.id
            && request.revision == self.req
            && request.sequence == self.playlist_open_seq
    }

    pub fn watch(&self) -> Option<&Watch> {
        match &self.kind {
            Kind::Watch(w) => Some(w),
            _ => None,
        }
    }

    pub fn watch_mut(&mut self) -> Option<&mut Watch> {
        match &mut self.kind {
            Kind::Watch(w) => Some(w),
            _ => None,
        }
    }

    /// Whether the navigation rail shows beside this page in a window `win_w` wide.
    pub fn has_rail(&self, win_w: f32) -> bool {
        !matches!(self.kind, Kind::Watch(_)) && win_w >= 792.0
    }

    pub fn has_chips(&self) -> bool {
        matches!(self.kind, Kind::Home { .. })
    }

    /// The plan for this geometry, recomputed only when it or the data changed.
    pub fn plan(&self, g: Geo) -> Rc<Plan> {
        let key = (g.key(), self.version.get());
        let plan = self.plan.borrow().clone();
        if plan.key == key {
            return plan;
        }
        let next = Rc::new(Plan {
            key,
            ..compute(self, g)
        });
        self.plan.replace(next.clone());
        next
    }
}

/// The window and scroller geometry a plan is laid out for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geo {
    pub win_w: f32,
    pub win_h: f32,
    /// The scroller's viewport.
    pub vw: f32,
    pub vh: f32,
    pub theatre: bool,
    pub show_resume: bool,
}

impl Geo {
    fn key(&self) -> [u32; 6] {
        [
            self.win_w.to_bits(),
            self.win_h.to_bits(),
            self.vw.to_bits(),
            self.vh.to_bits(),
            self.theatre as u32,
            self.show_resume as u32,
        ]
    }

    /// Content padding: 24, or 16 below 600.
    pub fn pad(&self) -> f32 {
        if self.win_w < 600.0 { 16.0 } else { 24.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum St {
    Error,
    Empty,
    MoreLoading,
    MoreError,
    TransLoading,
    TransError,
    CommentsLoading,
    CommentsError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum B {
    Resume,
    Card(usize),
    SkelCard(usize),
    Row(usize),
    SkelRow(usize),
    SearchControls,
    SearchEntry(usize),
    /// A search notice (index into `Search::notices`).
    Notice(usize),
    /// "Matches in video transcripts · search.pronsh.dev", then its rows.
    TransHead,
    TRow(usize),
    Compact(usize),
    SkelCompact(usize),
    Status(St),
    ChannelHead,
    Banner,
    Tabs,
    Strip,
    LibHead,
    Player,
    Info,
    SkelInfo,
    Desc,
    /// The chapter list under the open description.
    Chapters,
    CommentsHead,
    Comment(usize),
    Reply(usize, usize),
    RepliesMore(usize),
    RelatedChips,
    PlaylistHeading,
    PlaylistRow(usize),
    PlaylistMore,
    PlaylistCard(usize),
    PlaylistCardsMore,
    ChannelSort,
}

#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: B,
}

#[derive(Default)]
pub struct Plan {
    key: ([u32; 6], u64),
    /// Sorted by `y`.
    pub blocks: Vec<Block>,
    pub height: f32,
    max_h: f32,
    /// Grid cards: whether they show the channel (avatar and name).
    pub card_channel: bool,
    pub row_thumb: (f32, f32),
    /// The watch page's player box, in content coordinates, and where its comments start.
    pub player: [f32; 4],
    pub comments_top: f32,
    pub related_end: f32,
    pub playlist_end: f32,
    /// The description is clamped with an inline "…more" (or is open with "Show less").
    pub desc_more: bool,
    /// Where the infinite list ends (the trigger compares against this).
    pub list_end: f32,
    pub results_end: f32,
}

impl Plan {
    /// Blocks intersecting `[top, bottom)`.
    pub fn visible(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Block> {
        let from = top - self.max_h;
        let start = self.blocks.partition_point(|b| b.y < from);
        self.blocks
            .get(start..)
            .unwrap_or(&[])
            .iter()
            .take_while(move |b| b.y < bottom)
            .filter(move |b| b.y + b.h > top)
    }

    fn push(&mut self, b: Block) {
        self.max_h = self.max_h.max(b.h);
        self.height = self.height.max(b.y + b.h);
        self.blocks.push(b);
    }

    fn finish(&mut self, pad: f32) {
        self.blocks.sort_by(|a, b| a.y.total_cmp(&b.y));
        self.height += pad;
    }
}

pub const GRID_GAP: f32 = 16.0;
pub const GRID_ROW_GAP: f32 = 40.0;
/// Below the thumbnail: 12 + two title lines (44) + channel (20) + meta (20).
pub const CARD_TEXT: f32 = 96.0;
/// The same without the channel line (a channel's own grid).
pub const CARD_TEXT_NO_CHANNEL: f32 = 76.0;
pub const MORE_H: f32 = 64.0;
pub const ROW_GAP: f32 = 16.0;
pub const COMPACT_H: f32 = 94.0;
pub const COMPACT_GAP: f32 = 8.0;
pub const SEARCH_MAX: f32 = 1096.0;
pub const CHAPTERS_HEAD: f32 = 40.0;
pub const CHAPTER_ROW: f32 = 64.0;
pub const CHANNEL_MAX: f32 = 1284.0;
const SKELETONS: usize = 12;

/// Grid columns for a window `win_w` wide (spec v2).
pub fn grid_cols(win_w: f32) -> usize {
    match win_w {
        w if w < 600.0 => 1,
        w if w < 900.0 => 2,
        w if w < 1600.0 => 3,
        w if w < 2300.0 => 4,
        _ => 5,
    }
}

fn grid(p: &mut Plan, y0: f32, x0: f32, w: f32, cols: usize, n: usize, skeleton: bool) -> f32 {
    let cols = cols.max(1);
    let cw = ((w - GRID_GAP * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    let text = if p.card_channel {
        CARD_TEXT
    } else {
        CARD_TEXT_NO_CHANNEL
    };
    let ch = cw * 9.0 / 16.0 + text;
    for i in 0..n {
        let (r, c) = (i / cols, i % cols);
        p.push(Block {
            x: x0 + c as f32 * (cw + GRID_GAP),
            y: y0 + r as f32 * (ch + GRID_ROW_GAP),
            w: cw,
            h: ch,
            kind: if skeleton { B::SkelCard(i) } else { B::Card(i) },
        });
    }
    let rows = n.div_ceil(cols);
    if rows == 0 {
        y0
    } else {
        y0 + rows as f32 * (ch + GRID_ROW_GAP) - GRID_ROW_GAP
    }
}

/// Search-style rows: thumbnail clamp(240, 40 % of the width, 500), 16:9.
fn rows(p: &mut Plan, y0: f32, x0: f32, w: f32, n: usize, skeleton: bool) -> f32 {
    rows_of(p, y0, x0, w, n, if skeleton { B::SkelRow } else { B::Row })
}

fn row_thumb(w: f32) -> (f32, f32) {
    let width = (w * 0.4).clamp(240.0_f32.min(w * 0.45), 500.0).round();
    (width, (width * 9.0 / 16.0).round())
}

fn rows_of(p: &mut Plan, y0: f32, x0: f32, w: f32, n: usize, kind: fn(usize) -> B) -> f32 {
    let (tw, th) = row_thumb(w);
    p.row_thumb = (tw, th);
    for i in 0..n {
        p.push(Block {
            x: x0,
            y: y0 + i as f32 * (th + ROW_GAP),
            w,
            h: th,
            kind: kind(i),
        });
    }
    if n == 0 {
        y0
    } else {
        y0 + n as f32 * (th + ROW_GAP) - ROW_GAP
    }
}

/// The error, empty and load-more blocks for a feed; returns the y below them.
fn feed_status(p: &mut Plan, feed: &Feed, y: f32, x: f32, w: f32, vh: f32) -> f32 {
    let page_h = (vh - y - 24.0).max(320.0);
    match &feed.status {
        Load::Error(_) => {
            p.push(Block {
                x,
                y,
                w,
                h: page_h,
                kind: B::Status(St::Error),
            });
            return y + page_h;
        }
        Load::Idle if feed.items.is_empty() => {
            p.push(Block {
                x,
                y,
                w,
                h: page_h,
                kind: B::Status(St::Empty),
            });
            return y + page_h;
        }
        _ => {}
    }
    let kind = match feed.more {
        Load::Loading => St::MoreLoading,
        Load::Error(_) => St::MoreError,
        Load::Idle => return y,
    };
    p.push(Block {
        x,
        y: y + 16.0,
        w,
        h: MORE_H,
        kind: B::Status(kind),
    });
    y + 16.0 + MORE_H
}

/// The measuring width for a box `w` wide: rounded down to 16 points (see the module docs).
fn bucket(w: f32) -> f32 {
    ((w / 16.0).floor() * 16.0).max(16.0)
}

pub fn measure(text: &str, style: rune::TextStyle, w: f32) -> f32 {
    rune::text::measure(text, &[], style, Some(bucket(w))).1
}

/// Centers a box at most `max` wide in `[pad, vw - pad]`: (x, width).
fn centered(vw: f32, pad: f32, max: f32) -> (f32, f32) {
    let w = (vw - 2.0 * pad).max(100.0).min(max);
    (((vw - w) / 2.0).max(pad.min(vw / 2.0)), w)
}

fn compute(page: &Page, g: Geo) -> Plan {
    let mut p = Plan::default();
    let pad = g.pad();
    let x = pad;
    let w = (g.vw - 2.0 * pad).max(100.0);
    let mut y = if page.has_chips() { 0.0 } else { pad };
    let feed = &page.feed;
    let cols = grid_cols(g.win_w);
    p.card_channel = true;
    match &page.kind {
        Kind::Home { .. } => {
            y += 8.0;
            if g.show_resume {
                p.push(Block {
                    x,
                    y,
                    w: w.min(460.0),
                    h: 87.0,
                    kind: B::Resume,
                });
                y += 103.0;
            }
            let loading = feed.status == Load::Loading;
            let n = if loading { SKELETONS } else { feed.items.len() };
            let end = grid(&mut p, y, x, w, cols, n, loading);
            p.list_end = end;
            if !loading {
                feed_status(
                    &mut p,
                    feed,
                    if n == 0 { y } else { end + 24.0 },
                    x,
                    w,
                    g.vh,
                );
            }
        }
        Kind::Subs { channels } => {
            if channels.is_empty() {
                feed_status(
                    &mut p,
                    &Feed {
                        status: Load::Idle,
                        ..Feed::default()
                    },
                    y,
                    x,
                    w,
                    g.vh,
                );
            } else {
                p.push(Block {
                    x,
                    y,
                    w,
                    h: 118.0,
                    kind: B::Strip,
                });
                y += 118.0 + 16.0;
                let loading = feed.status == Load::Loading;
                let n = if loading { SKELETONS } else { feed.items.len() };
                let end = grid(&mut p, y, x, w, cols, n, loading);
                p.list_end = end;
                if !loading {
                    feed_status(
                        &mut p,
                        feed,
                        if n == 0 { y } else { end + 24.0 },
                        x,
                        w,
                        g.vh,
                    );
                }
            }
        }
        Kind::Channel {
            info, desc_open, ..
        } => {
            let (cx, cw) = centered(g.vw, pad, CHANNEL_MAX);
            if let Some(info) = info {
                if info.banner.is_some() {
                    let bh = (cw / 6.2).round();
                    p.push(Block {
                        x: cx,
                        y,
                        w: cw,
                        h: bh,
                        kind: B::Banner,
                    });
                    y += bh + 16.0;
                }
                let small = g.win_w < 600.0;
                let av = if small { 72.0 } else { 160.0 };
                let text_w = (cw - av - 24.0).max(120.0);
                let desc_h = if info.description.is_empty() {
                    0.0
                } else if *desc_open {
                    measure(&info.description, S14, text_w) + 8.0
                } else {
                    20.0 + 8.0
                };
                let name_h = measure(&info.name, S36B, text_w).min(88.0);
                let links_h = if *desc_open {
                    28.0 * info.links.len() as f32 + 28.0
                } else if info.links_label.is_some() || info.about.is_some() {
                    28.0
                } else {
                    0.0
                };
                let head = (name_h + 8.0 + 20.0 + desc_h + links_h + 12.0 + 36.0).max(av);
                p.push(Block {
                    x: cx,
                    y,
                    w: cw,
                    h: head,
                    kind: B::ChannelHead,
                });
                y += head + 8.0;
                p.push(Block {
                    x: cx,
                    y,
                    w: cw,
                    h: 48.0,
                    kind: B::Tabs,
                });
                y += 48.0 + 24.0;
            }
            if let Some(info) = info {
                if page.channel_tab == Some(nt_core::ChannelTab::Playlists)
                    && feed.status == Load::Idle
                {
                    let columns = cols.max(1);
                    let width =
                        ((cw - GRID_GAP * (columns as f32 - 1.0)) / columns as f32).max(1.0);
                    let height = width * 9.0 / 16.0 + 72.0;
                    for (i, _) in info.playlists.items.iter().enumerate() {
                        p.push(Block {
                            x: cx + (i % columns) as f32 * (width + GRID_GAP),
                            y: y + (i / columns) as f32 * (height + GRID_ROW_GAP),
                            w: width,
                            h: height,
                            kind: B::PlaylistCard(i),
                        });
                    }
                    p.list_end = y + info.playlists.items.len().div_ceil(columns) as f32
                        * (height + GRID_ROW_GAP);
                    if page.playlist_cards_more != Load::Idle {
                        p.push(Block {
                            x: cx,
                            y: p.list_end,
                            w: cw,
                            h: MORE_H,
                            kind: B::PlaylistCardsMore,
                        });
                    } else if info.playlists.items.is_empty() && info.playlists.next.is_none() {
                        feed_status(&mut p, feed, y, cx, cw, g.vh);
                    }
                    p.finish(pad);
                    return p;
                }
                if !info.video_sort.is_empty()
                    && page.channel_tab != Some(nt_core::ChannelTab::Home)
                {
                    p.push(Block {
                        x: cx,
                        y,
                        w: cw,
                        h: 42.0,
                        kind: B::ChannelSort,
                    });
                    y += 54.0;
                }
            }
            p.card_channel = false;
            let loading = feed.status == Load::Loading;
            let n = if loading { SKELETONS } else { feed.items.len() };
            let end = grid(&mut p, y, cx, cw, cols, n, loading);
            p.list_end = end;
            if !loading {
                feed_status(
                    &mut p,
                    feed,
                    if n == 0 { y } else { end + 24.0 },
                    cx,
                    cw,
                    g.vh,
                );
            }
        }
        Kind::Search(sr) => search_plan(&mut p, feed, sr, y, g, pad),
        Kind::History | Kind::Later => {
            let (cx, cw) = centered(g.vw, pad, SEARCH_MAX);
            p.push(Block {
                x: cx,
                y,
                w: cw,
                h: 44.0,
                kind: B::LibHead,
            });
            y += 44.0 + 24.0;
            let end = rows(&mut p, y, cx, cw, feed.items.len(), false);
            p.list_end = end;
            feed_status(
                &mut p,
                feed,
                if feed.items.is_empty() { y } else { end },
                cx,
                cw,
                g.vh,
            );
        }
        Kind::Watch(wt) => watch_plan(&mut p, page, wt, g),
    }
    p.finish(pad);
    p
}

/// The height of a search notice drawn `w` wide.
pub fn notice_h(n: &SearchNotice, w: f32) -> f32 {
    match n {
        SearchNotice::AgeGated { title, body } => {
            measure(title, S20B, w) + 8.0 + measure(body, S14, w) + 16.0
        }
        SearchNotice::Corrected { .. } => 20.0 + 4.0 + 20.0 + 16.0,
        SearchNotice::DidYouMean { .. } => 24.0 + 16.0,
        SearchNotice::NoResults => 76.0,
    }
}

fn search_plan(p: &mut Plan, feed: &Feed, sr: &Search, mut y: f32, g: Geo, pad: f32) {
    let (cx, cw) = centered(g.vw, pad, SEARCH_MAX);
    let loading = feed.status == Load::Loading;
    if !sr.chips.is_empty() || !sr.filters.is_empty() {
        p.push(Block {
            x: cx,
            y,
            w: cw,
            h: 44.0,
            kind: B::SearchControls,
        });
        y += 54.0;
    }
    if !loading {
        for (i, n) in sr.notices.iter().enumerate() {
            if matches!(n, SearchNotice::NoResults) && sr.trans.items.is_empty() {
                continue;
            }
            let h = notice_h(n, cw);
            if h > 0.0 {
                p.push(Block {
                    x: cx,
                    y,
                    w: cw,
                    h,
                    kind: B::Notice(i),
                });
                y += h + 8.0;
            }
        }
    }
    let n = sr.entries.len();
    if loading {
        rows(p, y, cx, cw, 6, true);
        return;
    }
    let (tw, th) = row_thumb(cw);
    p.row_thumb = (tw, th);
    for (i, entry) in sr.entries.iter().enumerate() {
        let h = match entry {
            nt_core::SearchResult::Shelf(_) => 40.0,
            nt_core::SearchResult::Channel(_) => th.min(150.0),
            _ => th,
        };
        p.push(Block {
            x: cx,
            y,
            w: cw,
            h,
            kind: B::SearchEntry(i),
        });
        y += h + ROW_GAP;
    }
    let end = y;
    p.list_end = end;
    p.results_end = end;
    let trans = &sr.trans;
    let gated = sr
        .notices
        .iter()
        .any(|n| matches!(n, SearchNotice::AgeGated { .. }));
    if !trans.items.is_empty() {
        // The results' own "more" spinner or error, then the transcript section.
        let top = if n == 0 {
            y
        } else {
            feed_status(p, feed, end + 8.0, cx, cw, g.vh) + 24.0
        };
        p.push(Block {
            x: cx,
            y: top,
            w: cw,
            h: 48.0,
            kind: B::TransHead,
        });
        let end2 = rows_of(p, top + 48.0 + 8.0, cx, cw, trans.items.len(), B::TRow);
        p.list_end = end2;
        let st = match trans.more {
            Load::Loading => Some(St::TransLoading),
            Load::Error(_) => Some(St::TransError),
            Load::Idle => None,
        };
        if let Some(st) = st {
            p.push(Block {
                x: cx,
                y: end2 + 16.0,
                w: cw,
                h: MORE_H,
                kind: B::Status(st),
            });
        }
    } else if n > 0 && feed.items.is_empty() && feed.status == Load::Idle {
        if feed.more != Load::Idle {
            let state = if feed.more == Load::Loading {
                St::MoreLoading
            } else {
                St::MoreError
            };
            p.push(Block {
                x: cx,
                y: end,
                w: cw,
                h: MORE_H,
                kind: B::Status(state),
            });
        }
    } else if !(gated && n == 0 && feed.status == Load::Idle) {
        feed_status(p, feed, end, cx, cw, g.vh);
    }
}

/// Where the watch page's parts go (spec v2: YouTube's formula).
#[derive(Clone, Copy, Debug)]
pub struct WatchGeo {
    pub two_cols: bool,
    /// Primary column x and width; secondary column x and width (two columns only).
    pub px: f32,
    pub pw: f32,
    pub sx: f32,
    pub sw: f32,
    /// The player box in content coordinates.
    pub player: [f32; 4],
    /// Where the columns start.
    pub top: f32,
}

pub fn watch_geo(g: Geo) -> WatchGeo {
    let (vw, win_h) = (g.vw, g.win_h);
    let pad = if vw < 600.0 { 16.0 } else { 24.0 };
    let avail = (vw - 2.0 * pad).max(100.0);
    let pmax = ((win_h - 169.0) * 16.0 / 9.0).max(320.0);
    let two_cols = vw >= 1017.0;
    let (pw, sw) = if two_cols {
        let sw = (avail * 0.29).clamp(300.0, 402.0);
        ((avail - sw - 24.0).min(pmax), sw)
    } else {
        (avail.min(pmax.max(avail.min(640.0))), 0.0)
    };
    let block = if two_cols { pw + 24.0 + sw } else { pw };
    let px = ((vw - block) / 2.0).max(pad.min(vw / 2.0));
    let sx = px + pw + 24.0;
    if g.theatre {
        let th = (vw * 0.5625).min(win_h - 169.0).max(180.0).round();
        WatchGeo {
            two_cols,
            px,
            pw,
            sx,
            sw,
            player: [0.0, 0.0, vw, th],
            top: th + 24.0,
        }
    } else {
        let ph = (pw * 9.0 / 16.0).round();
        WatchGeo {
            two_cols,
            px,
            pw,
            sx,
            sw,
            player: [px, pad, pw, ph],
            top: pad,
        }
    }
}

/// The owner row wraps its actions under the channel below this primary width.
pub const OWNER_WRAP: f32 = 720.0;

/// The collapsed description's cut: the byte index where "…more" goes so that the text plus it
/// fits three lines, or None when the whole description fits.
fn desc_cut(text: &str, w: f32) -> Option<usize> {
    let w = bucket(w);
    let fits = |s: &str| rune::text::measure(s, &[], S14, Some(w)).1 <= 60.5;
    if fits(text) {
        return None;
    }
    // Binary search over char boundaries for the longest prefix that fits with the suffix.
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, bounds.len());
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let cut = bounds.get(mid).copied().unwrap_or(text.len());
        let candidate = format!("{}…more", text.get(..cut).unwrap_or("").trim_end());
        if fits(&candidate) { lo = mid } else { hi = mid }
    }
    Some(bounds.get(lo).copied().unwrap_or(0))
}

pub fn description_cut(wt: &Watch, w: f32) -> Option<usize> {
    let key = bucket(w).to_bits();
    if let Some((k, cut)) = *wt.desc_cut.borrow()
        && k == key
    {
        return cut;
    }
    let cut = wt.info.as_ref().and_then(|i| desc_cut(&i.description, w));
    *wt.desc_cut.borrow_mut() = Some((key, cut));
    cut
}

/// The text column of a comment in a primary column `pw` wide.
pub fn comment_text_w(pw: f32) -> f32 {
    (pw - 56.0).max(60.0)
}

fn watch_plan(p: &mut Plan, page: &Page, wt: &Watch, g: Geo) {
    let wg = watch_geo(g);
    let (x, pw) = (wg.px, wg.pw);
    p.player = wg.player;
    p.push(Block {
        x: wg.player[0],
        y: wg.player[1],
        w: wg.player[2],
        h: wg.player[3],
        kind: B::Player,
    });
    let mut y = if g.theatre {
        wg.top
    } else {
        wg.player[1] + wg.player[3] + 12.0
    };
    let title = wt
        .info
        .as_ref()
        .map(|i| i.title.as_str())
        .unwrap_or(&wt.video.title);
    let title_h = measure(title, S20B, pw).max(28.0);
    let wrap = pw < OWNER_WRAP;
    let info_h = title_h + 12.0 + 40.0 + if wrap { 12.0 + 36.0 } else { 0.0 };
    p.push(Block {
        x,
        y,
        w: pw,
        h: info_h,
        kind: B::Info,
    });
    y += info_h + 16.0;
    let related_below = !wg.two_cols;
    match (&wt.info, &wt.error) {
        (Some(info), _) => {
            let inner = pw - 24.0;
            let text_h = if info.description.is_empty() {
                0.0
            } else if wt.desc_open {
                p.desc_more = true;
                measure(&info.description, S14, inner) + 20.0 + 8.0
            } else {
                p.desc_more = description_cut(wt, inner).is_some();
                measure(&info.description, S14, inner).min(60.0)
            };
            let h = 12.0 + 20.0 + if text_h > 0.0 { 4.0 + text_h } else { 0.0 } + 12.0;
            p.push(Block {
                x,
                y,
                w: pw,
                h,
                kind: B::Desc,
            });
            y += h + 24.0;
            if wt.desc_open && !info.chapters.is_empty() {
                let h = CHAPTERS_HEAD + info.chapters.len() as f32 * CHAPTER_ROW;
                p.push(Block {
                    x,
                    y,
                    w: pw,
                    h,
                    kind: B::Chapters,
                });
                y += h + 24.0;
            }
            if related_below {
                y = related(p, page, x, y, pw) + 24.0;
            }
            p.comments_top = y;
            p.push(Block {
                x,
                y,
                w: pw,
                h: 52.0,
                kind: B::CommentsHead,
            });
            y += 52.0 + 8.0;
            let tw = comment_text_w(pw);
            let wbits = bucket(tw).to_bits();
            let mut cache = wt.comment_h.borrow_mut();
            cache.truncate(wt.comments.len());
            for (i, c) in wt.comments.iter().enumerate() {
                let full = match cache.get(i) {
                    Some((b, h)) if *b == wbits => *h,
                    _ => {
                        let h = measure(&c.text, S14, tw).max(20.0);
                        if let Some(slot) = cache.get_mut(i) {
                            *slot = (wbits, h);
                        } else {
                            cache.push((wbits, h));
                        }
                        h
                    }
                };
                let open = wt.comments_open.contains(&c.id);
                let clamped = full > 80.5;
                let text_h = if open || !clamped { full } else { 80.0 };
                let more = if clamped { 4.0 + 20.0 } else { 0.0 };
                let pinned = if c.pinned { 22.0 } else { 0.0 };
                let reply_toggle = if c.replies.is_some() { 36.0 } else { 0.0 };
                let h = 18.0 + 2.0 + text_h + more + 8.0 + 32.0 + pinned + reply_toggle;
                p.push(Block {
                    x,
                    y,
                    w: pw,
                    h,
                    kind: B::Comment(i),
                });
                y += h + 16.0;
                if let Some(thread) = wt.replies.get(&c.id).filter(|reply| reply.open) {
                    let width = pw - 56.0;
                    for (j, reply) in thread.items.iter().enumerate() {
                        let height = measure(&reply.text, S14, comment_text_w(width)).max(20.0)
                            + 64.0
                            + if reply.pinned { 22.0 } else { 0.0 };
                        p.push(Block {
                            x: x + 56.0,
                            y,
                            w: width,
                            h: height,
                            kind: B::Reply(i, j),
                        });
                        y += height;
                    }
                    if thread.next.is_some() {
                        p.push(Block {
                            x: x + 56.0,
                            y,
                            w: width,
                            h: 40.0,
                            kind: B::RepliesMore(i),
                        });
                        y += 48.0;
                    }
                }
            }
            p.list_end = y;
            let st = match wt.comments_state {
                Load::Loading => Some(St::CommentsLoading),
                Load::Error(_) => Some(St::CommentsError),
                Load::Idle => None,
            };
            if let Some(st) = st {
                p.push(Block {
                    x,
                    y,
                    w: pw,
                    h: MORE_H,
                    kind: B::Status(st),
                });
            }
        }
        (None, Some(_)) => {
            p.push(Block {
                x,
                y,
                w: pw,
                h: 200.0,
                kind: B::Status(St::Error),
            });
            y += 224.0;
            if related_below {
                related(p, page, x, y, pw);
            }
        }
        (None, None) => {
            p.push(Block {
                x,
                y,
                w: pw,
                h: 120.0,
                kind: B::SkelInfo,
            });
        }
    }
    if wg.two_cols {
        related(p, page, wg.sx, wg.top, wg.sw);
    }
}

/// The related list (or its skeleton while the watch info loads); returns the y below it.
fn related(p: &mut Plan, page: &Page, x: f32, mut y: f32, w: f32) -> f32 {
    let loading = page
        .watch()
        .is_some_and(|w| w.info.is_none() && w.error.is_none());
    if let Some(watch) = page.watch()
        && let Some(panel) = &watch.playlist
    {
        p.push(Block {
            x,
            y,
            w,
            h: 42.0,
            kind: B::PlaylistHeading,
        });
        y += 44.0;
        for (i, _) in panel.items.iter().enumerate() {
            p.push(Block {
                x,
                y,
                w,
                h: 62.0,
                kind: B::PlaylistRow(i),
            });
            y += 64.0;
        }
        p.playlist_end = y;
        if watch.playlist_more != Load::Idle {
            p.push(Block {
                x,
                y,
                w,
                h: MORE_H,
                kind: B::PlaylistMore,
            });
            y += MORE_H;
        }
        y += 12.0;
    }
    if page
        .watch()
        .and_then(|watch| watch.info.as_ref())
        .is_some_and(|info| !info.related_chips.is_empty())
    {
        p.push(Block {
            x,
            y,
            w,
            h: 44.0,
            kind: B::RelatedChips,
        });
        y += 52.0;
    }
    let n = if loading { 8 } else { page.feed.items.len() };
    for i in 0..n {
        let kind = if loading {
            B::SkelCompact(i)
        } else {
            B::Compact(i)
        };
        p.push(Block {
            x,
            y,
            w,
            h: COMPACT_H,
            kind,
        });
        y += COMPACT_H + COMPACT_GAP;
    }
    p.related_end = y;
    if page.feed.more != Load::Idle {
        y = feed_status(p, &page.feed, y, x, w, 0.0);
    }
    y
}

/// Positions remembered for watched videos this session: fraction played, for the red bar
/// along the bottom of a thumbnail.
#[derive(Default)]
pub struct Progress(HashMap<String, f32>);

impl Progress {
    pub fn get(&self, id: &str) -> Option<f32> {
        self.0.get(id).copied()
    }

    pub fn set(&mut self, id: &str, frac: f32) {
        if self.0.len() > 2000 {
            self.0.clear();
        }
        self.0.insert(id.to_string(), frac.clamp(0.0, 1.0));
    }
}
