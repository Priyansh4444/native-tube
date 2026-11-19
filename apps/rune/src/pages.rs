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
use nt_core::{ChannelInfo, ChannelRef, Comment, Continuation, Video, WatchInfo};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

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
    pub comments_started: bool,
    pub desc_open: bool,
    /// Comments expanded past four lines ("Read more").
    pub comments_open: HashSet<String>,
    /// The viewer's own like / dislike (local only: the app never signs in).
    pub rating: Option<bool>,
    /// Playback position to restore when the page shows again (Back).
    pub resume_at: Option<f64>,
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
            comments_started: false,
            desc_open: false,
            comments_open: HashSet::new(),
            rating: None,
            resume_at: None,
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

// A handful of pages exist at a time; boxing the big variants would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    Home {
        topic: usize,
    },
    Search {
        query: String,
    },
    Watch(Box<Watch>),
    Channel {
        id: String,
        info: Option<ChannelInfo>,
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
    pub scroll: Scroller,
    /// Bumped whenever something the plan depends on changes.
    pub version: Cell<u64>,
    /// The request token: bumped by every (re)load of the page's data. An answer carries the
    /// token it was asked with and is dropped unless it still matches.
    pub req: u64,
    pub plan: RefCell<Plan>,
}

impl Page {
    pub fn new(kind: Kind) -> Page {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Page {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            feed: Feed::default(),
            scroll: Scroller::default(),
            version: Cell::new(1),
            req: 0,
            plan: RefCell::new(Plan::default()),
        }
    }

    pub fn touch(&self) {
        self.version.set(self.version.get() + 1);
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
    pub fn plan(&self, g: Geo) -> std::cell::Ref<'_, Plan> {
        let key = (g.key(), self.version.get());
        // Never a borrow panic: if the plan is in use (a caller still holds it), the new one waits
        // for the next call. Only this function borrows it mutably, and only for the store.
        let stale = self.plan.try_borrow().is_ok_and(|p| p.key != key);
        if stale {
            let p = compute(self, g);
            if let Ok(mut slot) = self.plan.try_borrow_mut() {
                *slot = Plan { key, ..p };
            }
        }
        self.plan.borrow()
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
}

impl Geo {
    fn key(&self) -> [u32; 5] {
        [
            self.win_w.to_bits(),
            self.win_h.to_bits(),
            self.vw.to_bits(),
            self.vh.to_bits(),
            self.theatre as u32,
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
    CommentsLoading,
    CommentsError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum B {
    Card(usize),
    SkelCard(usize),
    Row(usize),
    SkelRow(usize),
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
    CommentsHead,
    Comment(usize),
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
    key: ([u32; 5], u64),
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
    /// The description is clamped with an inline "…more" (or is open with "Show less").
    pub desc_more: bool,
    /// Where the infinite list ends (the trigger compares against this).
    pub list_end: f32,
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
    let tw = (w * 0.4).clamp(240.0_f32.min(w * 0.45), 500.0).round();
    let th = (tw * 9.0 / 16.0).round();
    p.row_thumb = (tw, th);
    for i in 0..n {
        p.push(Block {
            x: x0,
            y: y0 + i as f32 * (th + ROW_GAP),
            w,
            h: th,
            kind: if skeleton { B::SkelRow(i) } else { B::Row(i) },
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
                let head = (name_h + 8.0 + 20.0 + desc_h + 12.0 + 36.0).max(av);
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
        Kind::Search { .. } => {
            let (cx, cw) = centered(g.vw, pad, SEARCH_MAX);
            let loading = feed.status == Load::Loading;
            let n = if loading { 6 } else { feed.items.len() };
            let end = rows(&mut p, y, cx, cw, n, loading);
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
                let h = 18.0 + 2.0 + text_h + more + 8.0 + 32.0;
                p.push(Block {
                    x,
                    y,
                    w: pw,
                    h,
                    kind: B::Comment(i),
                });
                y += h + 16.0;
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
