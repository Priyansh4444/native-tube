//! Pages on the navigation stack and their layout plans.
//!
//! A plan is the page's content laid out as absolutely placed blocks with known sizes
//! (computed from the width, fixed row metrics and `rune::text::measure` for wrapped text).
//! The view builds only the blocks inside the scroller's mounted range, so nothing a frame
//! or a commit does scales with the number of loaded items beyond a binary search.

use crate::scroller::Scroller;
use crate::theme::*;
use nt_core::{ChannelInfo, ChannelRef, Comment, Continuation, Video, WatchInfo};
use std::cell::{Cell, RefCell};

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
}

impl Default for Feed {
    fn default() -> Self {
        Feed { items: Vec::new(), next: None, status: Load::Loading, more: Load::Idle }
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
    /// Playback position to restore when the page shows again (Back).
    pub resume_at: Option<f64>,
    /// Measured comment text heights: (width bits, height).
    pub comment_h: RefCell<Vec<(u32, f32)>>,
}

pub enum Kind {
    Home { topic: usize },
    Search { query: String },
    Watch(Box<Watch>),
    Channel { id: String, info: Option<ChannelInfo> },
    Subs { channels: Vec<ChannelRef> },
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

    /// The plan for this width and window height, recomputed only when either or the data changed.
    pub fn plan(&self, vw: f32, wh: f32) -> std::cell::Ref<'_, Plan> {
        let key = (vw.to_bits(), wh.to_bits(), self.version.get());
        if self.plan.borrow().key != key {
            let p = compute(self, vw, wh);
            *self.plan.borrow_mut() = Plan { key, ..p };
        }
        self.plan.borrow()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum St {
    Loading,
    Error,
    Empty,
    MoreLoading,
    MoreError,
    CommentsLoading,
    CommentsError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum B {
    Chips,
    Card(usize),
    Row(usize),
    Compact(usize),
    Status(St),
    ChannelHead,
    Banner,
    Strip,
    LibHead,
    Player,
    Info,
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
    key: (u32, u32, u64),
    /// Sorted by `y`.
    pub blocks: Vec<Block>,
    pub height: f32,
    max_h: f32,
    /// Grid card metrics (for thumbnails' pixel widths).
    pub card_w: f32,
    pub row_thumb: (f32, f32),
    /// The watch page's player box and where its comments start.
    pub player: [f32; 4],
    pub comments_top: f32,
    /// Where the infinite list ends (the trigger compares against this).
    pub list_end: f32,
}

impl Plan {
    /// Blocks intersecting `[top, bottom)`.
    pub fn visible(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Block> {
        let from = top - self.max_h;
        let start = self.blocks.partition_point(|b| b.y < from);
        self.blocks[start..].iter().take_while(move |b| b.y < bottom).filter(move |b| b.y + b.h > top)
    }

    fn push(&mut self, b: Block) {
        self.max_h = self.max_h.max(b.h);
        self.height = self.height.max(b.y + b.h);
        self.blocks.push(b);
    }

    fn finish(&mut self) {
        self.blocks.sort_by(|a, b| a.y.total_cmp(&b.y));
        self.height += PAD;
    }
}

pub const GRID_GAP: f32 = 16.0;
pub const GRID_ROW_GAP: f32 = 32.0;
pub const CARD_TEXT: f32 = 100.0;
pub const CHIPS_H: f32 = 32.0;
pub const STATUS_H: f32 = 160.0;
pub const MORE_H: f32 = 64.0;
pub const ROW_GAP: f32 = 16.0;
pub const COMPACT_H: f32 = 94.0;
pub const COMPACT_GAP: f32 = 8.0;
pub const RELATED_W: f32 = 400.0;

/// Columns and card width for a grid `w` wide: as many as fit with cards 300 to 420 wide.
pub fn grid_metrics(w: f32) -> (usize, f32) {
    let mut cols = (((w + GRID_GAP) / (300.0 + GRID_GAP)).floor() as usize).max(1);
    let card = |c: usize| (w - GRID_GAP * (c as f32 - 1.0)) / c as f32;
    while card(cols) > 420.0 {
        cols += 1;
    }
    (cols, card(cols).max(1.0))
}

fn grid(p: &mut Plan, y0: f32, x0: f32, w: f32, n: usize) -> f32 {
    let (cols, cw) = grid_metrics(w);
    p.card_w = cw;
    let ch = cw * 9.0 / 16.0 + CARD_TEXT;
    for i in 0..n {
        let (r, c) = (i / cols, i % cols);
        p.push(Block { x: x0 + c as f32 * (cw + GRID_GAP), y: y0 + r as f32 * (ch + GRID_ROW_GAP), w: cw, h: ch, kind: B::Card(i) });
    }
    let rows = n.div_ceil(cols);
    if rows == 0 { y0 } else { y0 + rows as f32 * (ch + GRID_ROW_GAP) - GRID_ROW_GAP }
}

fn rows(p: &mut Plan, y0: f32, x0: f32, w: f32, n: usize) -> f32 {
    let tw = if w >= 760.0 { 360.0 } else { (w * 0.42).max(140.0) };
    let th = (tw * 202.0 / 360.0).round();
    p.row_thumb = (tw, th);
    for i in 0..n {
        p.push(Block { x: x0, y: y0 + i as f32 * (th + ROW_GAP), w, h: th, kind: B::Row(i) });
    }
    if n == 0 { y0 } else { y0 + n as f32 * (th + ROW_GAP) - ROW_GAP }
}

/// The first-load, error, empty and load-more status blocks for a feed.
fn feed_status(p: &mut Plan, feed: &Feed, y: f32, x: f32, w: f32, empty: bool) -> f32 {
    match &feed.status {
        Load::Loading => {
            p.push(Block { x, y, w, h: STATUS_H, kind: B::Status(St::Loading) });
            return y + STATUS_H;
        }
        Load::Error(_) => {
            p.push(Block { x, y, w, h: STATUS_H, kind: B::Status(St::Error) });
            return y + STATUS_H;
        }
        Load::Idle => {}
    }
    if empty && feed.items.is_empty() {
        p.push(Block { x, y, w, h: STATUS_H, kind: B::Status(St::Empty) });
        return y + STATUS_H;
    }
    match feed.more {
        Load::Loading => {
            p.push(Block { x, y: y + 8.0, w, h: MORE_H, kind: B::Status(St::MoreLoading) });
            y + 8.0 + MORE_H
        }
        Load::Error(_) => {
            p.push(Block { x, y: y + 8.0, w, h: MORE_H, kind: B::Status(St::MoreError) });
            y + 8.0 + MORE_H
        }
        Load::Idle => y,
    }
}

fn measure(text: &str, style: rune::TextStyle, w: f32) -> f32 {
    rune::text::measure(text, &[], style, Some(w.max(10.0))).1
}

fn compute(page: &Page, vw: f32, wh: f32) -> Plan {
    let mut p = Plan::default();
    let x = PAD;
    let w = (vw - 2.0 * PAD).max(100.0);
    let mut y = PAD;
    let feed = &page.feed;
    match &page.kind {
        Kind::Home { .. } => {
            p.push(Block { x, y, w, h: CHIPS_H, kind: B::Chips });
            y += CHIPS_H + PAD;
            let end = grid(&mut p, y, x, w, feed.items.len());
            p.list_end = end;
            feed_status(&mut p, feed, if feed.items.is_empty() { y } else { end + PAD }, x, w, true);
        }
        Kind::Subs { channels } => {
            if channels.is_empty() {
                p.push(Block { x, y, w, h: STATUS_H, kind: B::Status(St::Empty) });
            } else {
                p.push(Block { x, y, w, h: 112.0, kind: B::Strip });
                y += 112.0 + PAD;
                let end = grid(&mut p, y, x, w, feed.items.len());
                p.list_end = end;
                feed_status(&mut p, feed, if feed.items.is_empty() { y } else { end + PAD }, x, w, true);
            }
        }
        Kind::Channel { info, .. } => {
            if let Some(info) = info {
                if info.banner.is_some() {
                    p.push(Block { x, y, w, h: 160.0, kind: B::Banner });
                    y += 160.0 + PAD;
                }
                p.push(Block { x, y, w, h: 112.0, kind: B::ChannelHead });
                y += 112.0 + PAD;
            }
            let end = grid(&mut p, y, x, w, feed.items.len());
            p.list_end = end;
            feed_status(&mut p, feed, if feed.items.is_empty() { y } else { end + PAD }, x, w, true);
        }
        Kind::Search { .. } => {
            let end = rows(&mut p, y, x, w.min(1100.0), feed.items.len());
            p.list_end = end;
            feed_status(&mut p, feed, if feed.items.is_empty() { y } else { end + PAD }, x, w.min(1100.0), true);
        }
        Kind::History | Kind::Later => {
            p.push(Block { x, y, w: w.min(1100.0), h: 40.0, kind: B::LibHead });
            y += 40.0 + PAD;
            let end = rows(&mut p, y, x, w.min(1100.0), feed.items.len());
            p.list_end = end;
            if feed.items.is_empty() {
                p.push(Block { x, y, w, h: STATUS_H, kind: B::Status(St::Empty) });
            }
        }
        Kind::Watch(wt) => watch_plan(&mut p, page, wt, vw, wh),
    }
    p.finish();
    p
}

/// Height of the description box for a primary column `pw` wide.
pub const DESC_LINES: usize = 3;

fn watch_plan(p: &mut Plan, page: &Page, wt: &Watch, vw: f32, wh: f32) {
    let x = PAD;
    let wide = vw >= 1000.0;
    let w = (vw - 2.0 * PAD).max(100.0);
    let pw = if wide { (w - RELATED_W - PAD).max(300.0) } else { w };
    let mut y = PAD;
    let ph = (pw * 9.0 / 16.0).min(wh * 0.7).round();
    p.player = [x, y, pw, ph];
    p.push(Block { x, y, w: pw, h: ph, kind: B::Player });
    y += ph + 12.0;
    let title = wt.info.as_ref().map(|i| i.title.as_str()).unwrap_or(&wt.video.title);
    let title_h = measure(title, S20, pw).max(28.0);
    let info_h = title_h + 12.0 + 40.0;
    p.push(Block { x, y, w: pw, h: info_h, kind: B::Info });
    y += info_h + 16.0;
    match (&wt.info, &wt.error) {
        (Some(info), _) => {
            let inner = pw - 24.0;
            let full = if info.description.is_empty() { 0.0 } else { measure(&info.description, S14, inner) };
            let clamped = full.min(20.0 * DESC_LINES as f32);
            let more = full > clamped + 1.0;
            let text_h = if wt.desc_open { full } else { clamped };
            let h = 12.0 + 20.0 + if text_h > 0.0 { 4.0 + text_h } else { 0.0 } + if more || wt.desc_open { 24.0 } else { 0.0 } + 12.0;
            p.push(Block { x, y, w: pw, h, kind: B::Desc });
            y += h + PAD;
            p.comments_top = y;
            p.push(Block { x, y, w: pw, h: 48.0, kind: B::CommentsHead });
            y += 48.0;
            let tw = pw - 52.0;
            let wbits = tw.to_bits();
            {
                let mut cache = wt.comment_h.borrow_mut();
                cache.truncate(wt.comments.len());
                for (i, c) in wt.comments.iter().enumerate() {
                    let th = match cache.get(i) {
                        Some((b, h)) if *b == wbits => *h,
                        _ => {
                            let h = measure(&c.text, S14, tw).max(20.0);
                            if i < cache.len() { cache[i] = (wbits, h) } else { cache.push((wbits, h)) }
                            h
                        }
                    };
                    let h = 18.0 + 4.0 + th + 6.0 + 18.0;
                    p.push(Block { x, y, w: pw, h, kind: B::Comment(i) });
                    y += h + 20.0;
                }
            }
            p.list_end = y;
            match wt.comments_state {
                Load::Loading => {
                    p.push(Block { x, y, w: pw, h: MORE_H, kind: B::Status(St::CommentsLoading) });
                    y += MORE_H;
                }
                Load::Error(_) => {
                    p.push(Block { x, y, w: pw, h: MORE_H, kind: B::Status(St::CommentsError) });
                    y += MORE_H;
                }
                Load::Idle => {}
            }
        }
        (None, Some(_)) => {
            p.push(Block { x, y, w: pw, h: STATUS_H, kind: B::Status(St::Error) });
            y += STATUS_H;
        }
        (None, None) => {
            p.push(Block { x, y, w: pw, h: STATUS_H, kind: B::Status(St::Loading) });
            y += STATUS_H;
        }
    }
    let (rx, mut ry) = if wide { (x + pw + PAD, PAD) } else { (x, y + PAD) };
    let rw = if wide { RELATED_W } else { pw };
    for i in 0..page.feed.items.len() {
        p.push(Block { x: rx, y: ry, w: rw, h: COMPACT_H, kind: B::Compact(i) });
        ry += COMPACT_H + COMPACT_GAP;
    }
}
