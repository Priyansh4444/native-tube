//! Page state and page layout.
//!
//! Every page's content is a list of absolutely positioned [`Block`]s whose heights are
//! known up front (fixed for cards and rows, measured once with GPUI's text shaper for
//! comments, titles and the description). The scroller then only has to binary-search
//! the blocks that intersect the viewport plus one viewport of overscan, so no per-frame
//! work scales with the number of loaded items.

use crate::theme::*;
use nt_core::scroll::ScrollAnimator;
use nt_core::{ChannelInfo, ChannelRef, Comment, Continuation, Video, WatchInfo};

pub type PageId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Failed(String),
    Ready,
}

#[derive(Default)]
pub struct Feed {
    pub items: Vec<Video>,
    pub next: Option<Continuation>,
    pub loading_more: bool,
    pub more_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    Home,
    Subscriptions,
    History,
    WatchLater,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommentsState {
    NotStarted,
    Loading,
    Idle,
    Failed(String),
}

pub struct WatchState {
    pub video: Video,
    pub info: Option<WatchInfo>,
    pub comments: Vec<Comment>,
    pub comments_next: Option<Continuation>,
    pub comments_total: Option<String>,
    pub comments_state: CommentsState,
    /// LoadMoreComments arrived before the watch info (and its comments cursor).
    pub want_comments: bool,
    pub desc_expanded: bool,
    pub subscribed: bool,
    pub saved: bool,
    /// Where to seek once the video is playing again after Back.
    pub resume_at: Option<f64>,
    /// Measured comment heights for `comment_h_width`.
    pub comment_h: Vec<f32>,
    pub comment_h_width: f32,
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
    Watch(Box<WatchState>),
    Channel {
        id: String,
        info: Option<ChannelInfo>,
        subscribed: bool,
    },
    Subscriptions {
        channels: Vec<ChannelRef>,
    },
    History,
    WatchLater,
}

pub struct Page {
    pub id: PageId,
    pub kind: Kind,
    pub status: Load,
    /// The page's main video list: home/search/channel/subscription grid, library list,
    /// or the related list on a watch page.
    pub feed: Feed,
    pub scroll: ScrollAnimator,
    /// Bumped whenever something that affects layout changes.
    pub version: u64,
    /// Bumped for every (re)load, so stale answers are dropped.
    pub req: u64,
    pub layout: Option<(LayoutKey, Layout)>,
}

impl Page {
    pub fn new(id: PageId, kind: Kind) -> Page {
        Page {
            id,
            kind,
            status: Load::Loading,
            feed: Feed::default(),
            scroll: ScrollAnimator::new(),
            version: 0,
            req: 0,
            layout: None,
        }
    }

    pub fn root(&self) -> Option<Root> {
        match self.kind {
            Kind::Home { .. } => Some(Root::Home),
            Kind::Subscriptions { .. } => Some(Root::Subscriptions),
            Kind::History => Some(Root::History),
            Kind::WatchLater => Some(Root::WatchLater),
            _ => None,
        }
    }

    pub fn touch(&mut self) {
        self.version += 1;
    }

    pub fn has_chips(&self) -> bool {
        matches!(self.kind, Kind::Home { .. })
    }

    pub fn watch(&self) -> Option<&WatchState> {
        match &self.kind {
            Kind::Watch(w) => Some(w),
            _ => None,
        }
    }

    pub fn watch_mut(&mut self) -> Option<&mut WatchState> {
        match &mut self.kind {
            Kind::Watch(w) => Some(w),
            _ => None,
        }
    }
}

// ---- geometry ------------------------------------------------------------------------------

/// Window geometry in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub win_w: f32,
    pub win_h: f32,
    pub scale: f32,
}

impl Geo {
    pub fn content_w(&self) -> f32 {
        (self.win_w - RAIL_W).max(100.0)
    }
    pub fn inner_w(&self) -> f32 {
        (self.content_w() - 2.0 * PAD).max(100.0)
    }
    pub fn scroller_top(&self, chips: bool) -> f32 {
        TOP_BAR_H + if chips { CHIPS_H } else { 0.0 }
    }
    pub fn viewport_h(&self, chips: bool) -> f32 {
        (self.win_h - self.scroller_top(chips)).max(50.0)
    }
    pub fn wide_watch(&self) -> bool {
        self.win_w >= 1000.0
    }
}

pub const COL_GAP: f32 = 16.0;
pub const ROW_GAP: f32 = 32.0;
pub const CARD_MIN: f32 = 300.0;
pub const CARD_MAX: f32 = 420.0;
/// Below the thumbnail: 12 gap, then title (2 x 22) + 4 + channel 20 + meta 20.
pub const CARD_TEXT_H: f32 = 12.0 + 44.0 + 4.0 + 20.0 + 20.0;

/// As many columns as fit with a card width between 300 and 420.
pub fn grid(inner_w: f32) -> (usize, f32) {
    let fit = ((inner_w + COL_GAP) / (CARD_MIN + COL_GAP))
        .floor()
        .max(1.0) as usize;
    let cols = fit.max(1);
    let w = ((inner_w - COL_GAP * (cols as f32 - 1.0)) / cols as f32).min(CARD_MAX);
    (cols, w)
}

pub fn card_h(card_w: f32) -> f32 {
    card_w * 9.0 / 16.0 + CARD_TEXT_H
}

/// Search-style list row: thumbnail 360x202 (narrower when the window is narrow).
pub fn list_thumb_w(inner_w: f32) -> f32 {
    if inner_w >= 760.0 {
        360.0
    } else {
        (inner_w * 0.45).floor()
    }
}
pub const LIST_GAP: f32 = 16.0;

pub const COMPACT_THUMB_W: f32 = 168.0;
pub const COMPACT_THUMB_H: f32 = 94.0;
pub const COMPACT_GAP: f32 = 8.0;
pub const RELATED_W: f32 = 400.0;

pub const SPINNER_H: f32 = 72.0;

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    GridRow {
        first: usize,
        count: usize,
        card_w: f32,
    },
    SkeletonRow {
        cols: usize,
        card_w: f32,
    },
    ListRow(usize),
    CompactRow(usize),
    Spinner,
    MoreError,
    LoadError,
    Empty(&'static str),
    PageTitle,
    ChannelBanner,
    ChannelHeader,
    SubsStrip,
    Player,
    WatchMeta {
        title_h: f32,
    },
    WatchDesc {
        lines: usize,
        shown: usize,
    },
    CommentsHeader,
    Comment(usize),
    CommentsSpinner,
    CommentsError,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutKey {
    pub geo: Geo,
    pub version: u64,
}

#[derive(Default, Debug)]
pub struct Layout {
    /// One or two columns of blocks, each sorted by `y`.
    pub columns: Vec<Vec<Block>>,
    pub content_h: f32,
    /// Watch page: where the comments section starts and the primary column ends.
    pub comments_y: Option<f32>,
    pub primary_end: f32,
    /// Where the main video list ends (infinite scroll triggers near it).
    pub main_end: f32,
    /// Index of the column that holds the page's main video list.
    pub main_column: usize,
}

impl Layout {
    /// Blocks of every column that intersect `[top, bottom)`.
    pub fn visible(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Block> {
        self.columns.iter().flat_map(move |col| {
            let start = col.partition_point(|b| b.y + b.h <= top);
            col[start..].iter().take_while(move |b| b.y < bottom)
        })
    }
}

/// Measures how many lines `text` wraps to at `font_size`/`bold` within `width`.
pub trait Measure {
    fn lines(&mut self, text: &str, font_size: f32, weight: u16, width: f32) -> usize;
}

pub const TITLE_LH: f32 = 28.0;
pub const BODY_LH: f32 = 20.0;
pub const DESC_CLAMP: usize = 3;

pub fn watch_columns(geo: &Geo) -> (f32, Option<f32>) {
    let inner = geo.inner_w();
    if geo.wide_watch() {
        let pw = (inner - RELATED_W - PAD).max(320.0);
        (pw, Some(PAD + pw + PAD))
    } else {
        (inner, None)
    }
}

pub fn player_h(geo: &Geo, pw: f32) -> f32 {
    (pw * 9.0 / 16.0).min(geo.win_h * 0.7).floor()
}

/// Comment block: avatar 40 + 12, then "@author • published" (18), the text, likes/replies (20).
pub fn comment_h(lines: usize) -> f32 {
    (18.0 + 4.0 + lines.max(1) as f32 * BODY_LH + 6.0 + 20.0).max(40.0) + 20.0
}

pub fn comment_text_w(pw: f32) -> f32 {
    (pw - 52.0).max(50.0)
}

pub fn build_layout(page: &mut Page, geo: &Geo, m: &mut dyn Measure) -> Layout {
    let inner = geo.inner_w();
    let mut lay = Layout {
        columns: vec![Vec::new()],
        ..Default::default()
    };
    let mut y = PAD;
    match &mut page.kind {
        Kind::Watch(w) => {
            let (pw, related_x) = watch_columns(geo);
            let ph = player_h(geo, pw);
            let mut col: Vec<Block> = Vec::new();
            col.push(Block {
                x: PAD,
                y,
                w: pw,
                h: ph,
                kind: BlockKind::Player,
            });
            y += ph;
            match (&page.status, &w.info) {
                (Load::Failed(_), _) => {
                    col.push(Block {
                        x: PAD,
                        y: y + 16.0,
                        w: pw,
                        h: 160.0,
                        kind: BlockKind::LoadError,
                    });
                    y += 176.0;
                }
                (_, None) => {
                    col.push(Block {
                        x: PAD,
                        y,
                        w: pw,
                        h: SPINNER_H,
                        kind: BlockKind::Spinner,
                    });
                    y += SPINNER_H;
                }
                (_, Some(info)) => {
                    let title_lines = m.lines(&info.title, 20.0, 600, pw).max(1);
                    let title_h = title_lines as f32 * TITLE_LH;
                    let meta_h = 12.0 + title_h + 12.0 + 48.0;
                    col.push(Block {
                        x: PAD,
                        y,
                        w: pw,
                        h: meta_h,
                        kind: BlockKind::WatchMeta { title_h },
                    });
                    y += meta_h + 12.0;
                    let lines = if info.description.trim().is_empty() {
                        0
                    } else {
                        m.lines(info.description.trim_end(), 14.0, 400, pw - 24.0)
                    };
                    let shown = if w.desc_expanded {
                        lines
                    } else {
                        lines.min(DESC_CLAMP)
                    };
                    let toggle = if lines > DESC_CLAMP { 4.0 + 20.0 } else { 0.0 };
                    let desc_h = 12.0
                        + 20.0
                        + if shown > 0 {
                            4.0 + shown as f32 * BODY_LH
                        } else {
                            0.0
                        }
                        + toggle
                        + 12.0;
                    col.push(Block {
                        x: PAD,
                        y,
                        w: pw,
                        h: desc_h,
                        kind: BlockKind::WatchDesc { lines, shown },
                    });
                    y += desc_h + 24.0;
                    if info.comments.is_some() || !w.comments.is_empty() {
                        lay.comments_y = Some(y);
                        col.push(Block {
                            x: PAD,
                            y,
                            w: pw,
                            h: 40.0,
                            kind: BlockKind::CommentsHeader,
                        });
                        y += 40.0 + 12.0;
                        let tw = comment_text_w(pw);
                        if w.comment_h_width != tw {
                            w.comment_h.clear();
                            w.comment_h_width = tw;
                        }
                        for i in w.comment_h.len()..w.comments.len() {
                            let lines = m.lines(w.comments[i].text.trim_end(), 14.0, 400, tw);
                            w.comment_h.push(comment_h(lines));
                        }
                        for (i, h) in w.comment_h.iter().enumerate() {
                            col.push(Block {
                                x: PAD,
                                y,
                                w: pw,
                                h: *h,
                                kind: BlockKind::Comment(i),
                            });
                            y += h;
                        }
                        match w.comments_state {
                            CommentsState::Loading => {
                                col.push(Block {
                                    x: PAD,
                                    y,
                                    w: pw,
                                    h: SPINNER_H,
                                    kind: BlockKind::CommentsSpinner,
                                });
                                y += SPINNER_H;
                            }
                            CommentsState::Failed(_) => {
                                col.push(Block {
                                    x: PAD,
                                    y,
                                    w: pw,
                                    h: 96.0,
                                    kind: BlockKind::CommentsError,
                                });
                                y += 96.0;
                            }
                            _ => {}
                        }
                    }
                }
            }
            lay.primary_end = y;
            // Related videos: their own column when wide, otherwise below the comments.
            let (rx, rw, mut ry, column) = match related_x {
                Some(x) => (x, RELATED_W, PAD, 1),
                None => (PAD, pw, y + 24.0, 0),
            };
            let mut rel: Vec<Block> = Vec::new();
            for i in 0..page.feed.items.len() {
                rel.push(Block {
                    x: rx,
                    y: ry,
                    w: rw,
                    h: COMPACT_THUMB_H,
                    kind: BlockKind::CompactRow(i),
                });
                ry += COMPACT_THUMB_H + COMPACT_GAP;
            }
            if page.feed.loading_more {
                rel.push(Block {
                    x: rx,
                    y: ry,
                    w: rw,
                    h: SPINNER_H,
                    kind: BlockKind::Spinner,
                });
                ry += SPINNER_H;
            } else if page.feed.more_error.is_some() {
                rel.push(Block {
                    x: rx,
                    y: ry,
                    w: rw,
                    h: 96.0,
                    kind: BlockKind::MoreError,
                });
                ry += 96.0;
            }
            if column == 1 {
                lay.columns = vec![col, rel];
                lay.main_column = 1;
            } else {
                col.extend(rel);
                lay.columns = vec![col];
            }
            lay.main_end = ry;
            lay.content_h = y.max(ry) + PAD;
            return lay;
        }
        Kind::Channel { info, .. } => {
            if let Some(info) = info {
                if info.banner.is_some() {
                    let bh = 160.0;
                    lay.columns[0].push(Block {
                        x: PAD,
                        y,
                        w: inner,
                        h: bh,
                        kind: BlockKind::ChannelBanner,
                    });
                    y += bh + 16.0;
                }
                let hh = 168.0;
                lay.columns[0].push(Block {
                    x: PAD,
                    y,
                    w: inner,
                    h: hh,
                    kind: BlockKind::ChannelHeader,
                });
                y += hh + 16.0;
            }
            y = grid_section(
                page.status.clone(),
                &page.feed,
                inner,
                y,
                &mut lay.columns[0],
                "This channel has no videos",
            );
        }
        Kind::Subscriptions { channels } => {
            if page.status == Load::Ready && channels.is_empty() {
                lay.columns[0].push(Block {
                    x: PAD,
                    y,
                    w: inner,
                    h: 200.0,
                    kind: BlockKind::Empty("You haven't subscribed to any channels yet"),
                });
                y += 200.0;
            } else {
                if !channels.is_empty() {
                    lay.columns[0].push(Block {
                        x: PAD,
                        y,
                        w: inner,
                        h: 112.0,
                        kind: BlockKind::SubsStrip,
                    });
                    y += 112.0 + 16.0;
                }
                y = grid_section(
                    page.status.clone(),
                    &page.feed,
                    inner,
                    y,
                    &mut lay.columns[0],
                    "No recent uploads",
                );
            }
        }
        Kind::Home { .. } => {
            y = grid_section(
                page.status.clone(),
                &page.feed,
                inner,
                y,
                &mut lay.columns[0],
                "No videos",
            );
        }
        Kind::Search { .. } | Kind::History | Kind::WatchLater => {
            let col = &mut lay.columns[0];
            if matches!(page.kind, Kind::History | Kind::WatchLater) {
                col.push(Block {
                    x: PAD,
                    y,
                    w: inner,
                    h: 48.0,
                    kind: BlockKind::PageTitle,
                });
                y += 48.0 + 8.0;
            }
            let empty = match page.kind {
                Kind::History => "Videos you watch will show up here",
                Kind::WatchLater => "Save videos to watch later and they'll show up here",
                _ => "No results",
            };
            match &page.status {
                Load::Loading => {
                    col.push(Block {
                        x: PAD,
                        y,
                        w: inner,
                        h: SPINNER_H * 2.0,
                        kind: BlockKind::Spinner,
                    });
                    y += SPINNER_H * 2.0;
                }
                Load::Failed(_) => {
                    col.push(Block {
                        x: PAD,
                        y,
                        w: inner,
                        h: 160.0,
                        kind: BlockKind::LoadError,
                    });
                    y += 160.0;
                }
                Load::Ready if page.feed.items.is_empty() => {
                    col.push(Block {
                        x: PAD,
                        y,
                        w: inner,
                        h: 200.0,
                        kind: BlockKind::Empty(empty),
                    });
                    y += 200.0;
                }
                Load::Ready => {
                    let th = list_thumb_w(inner) * 9.0 / 16.0;
                    let th = th.round();
                    for i in 0..page.feed.items.len() {
                        col.push(Block {
                            x: PAD,
                            y,
                            w: inner,
                            h: th,
                            kind: BlockKind::ListRow(i),
                        });
                        y += th + LIST_GAP;
                    }
                    y = feed_tail(&page.feed, inner, y, col);
                }
            }
        }
    }
    lay.content_h = y + PAD;
    lay.primary_end = lay.content_h;
    lay.main_end = lay.content_h;
    lay
}

fn feed_tail(feed: &Feed, inner: f32, mut y: f32, col: &mut Vec<Block>) -> f32 {
    if feed.loading_more {
        col.push(Block {
            x: PAD,
            y,
            w: inner,
            h: SPINNER_H,
            kind: BlockKind::Spinner,
        });
        y += SPINNER_H;
    } else if feed.more_error.is_some() {
        col.push(Block {
            x: PAD,
            y,
            w: inner,
            h: 96.0,
            kind: BlockKind::MoreError,
        });
        y += 96.0;
    }
    y
}

fn grid_section(
    status: Load,
    feed: &Feed,
    inner: f32,
    mut y: f32,
    col: &mut Vec<Block>,
    empty: &'static str,
) -> f32 {
    let (cols, cw) = grid(inner);
    let ch = card_h(cw);
    match status {
        Load::Loading => {
            for _ in 0..3 {
                col.push(Block {
                    x: PAD,
                    y,
                    w: inner,
                    h: ch,
                    kind: BlockKind::SkeletonRow { cols, card_w: cw },
                });
                y += ch + ROW_GAP;
            }
        }
        Load::Failed(_) => {
            col.push(Block {
                x: PAD,
                y,
                w: inner,
                h: 160.0,
                kind: BlockKind::LoadError,
            });
            y += 160.0;
        }
        Load::Ready if feed.items.is_empty() => {
            col.push(Block {
                x: PAD,
                y,
                w: inner,
                h: 200.0,
                kind: BlockKind::Empty(empty),
            });
            y += 200.0;
        }
        Load::Ready => {
            let mut first = 0;
            while first < feed.items.len() {
                let count = cols.min(feed.items.len() - first);
                col.push(Block {
                    x: PAD,
                    y,
                    w: inner,
                    h: ch,
                    kind: BlockKind::GridRow {
                        first,
                        count,
                        card_w: cw,
                    },
                });
                y += ch + ROW_GAP;
                first += count;
            }
            y = feed_tail(feed, inner, y, col);
        }
    }
    y
}
