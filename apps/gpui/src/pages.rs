//! Page state and page layout.
//!
//! Every page's content is a list of absolutely positioned [`Block`]s whose heights are known
//! before drawing: fixed for thumbnails and rows, measured once with GPUI's text shaper for
//! titles, the description and comments (at bucketed widths, so a resize re-measures only every
//! few pixels). The scroller binary-searches the blocks that intersect the viewport plus a band
//! of overscan, so no per-frame work grows with the number of loaded items.

use nt_core::scroll::ScrollAnimator;
use nt_core::{
    ChannelInfo, ChannelRef, ChannelTab, Chip, Comment, CommentSort, Continuation, PlaylistRef,
    SearchChannel, SearchFilter, SearchFilterGroup, SearchNotice, SearchOption, SearchPage,
    SearchResult, Video, WatchInfo,
};
use std::collections::{HashMap, HashSet};

pub type PageId = u64;

/// What the user sees when something fails. The raw error goes to the log only.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub title: &'static str,
    pub detail: &'static str,
    pub offline: bool,
}

impl Problem {
    pub fn from_error(e: &nt_core::Error, what: &str) -> Problem {
        eprintln!("nt-gpui: {what} failed: {e}");
        use nt_core::Error as E;
        match e {
            E::Network(_) => Problem {
                title: "Couldn't reach YouTube",
                detail: "Check your connection and try again.",
                offline: true,
            },
            E::Fixture(_) => Problem {
                title: "Something went wrong",
                detail: "This page isn't in the recorded test data.",
                offline: false,
            },
            E::Player(_) => Problem {
                title: "This video is unavailable",
                detail: "It can't be played here. Try another video.",
                offline: false,
            },
            _ => Problem {
                title: "Something went wrong",
                detail: "YouTube sent an answer this app doesn't understand.",
                offline: false,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Failed(Problem),
    Ready,
}

#[derive(Default)]
pub struct Feed {
    pub items: Vec<Video>,
    pub next: Option<Continuation>,
    pub loading_more: bool,
    pub more_error: Option<Problem>,
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
    Failed(Problem),
}

pub struct Replies {
    pub expanded: bool,
    pub items: Vec<Comment>,
    pub next: Option<Continuation>,
    pub status: CommentsState,
}

pub struct WatchState {
    pub video: Video,
    pub playlist: Option<String>,
    pub playlist_loading: bool,
    pub playlist_error: bool,
    pub info: Option<WatchInfo>,
    pub comments: Vec<Comment>,
    pub comments_next: Option<Continuation>,
    pub comments_total: Option<String>,
    pub comments_sort: Vec<CommentSort>,
    pub comments_replace: Option<usize>,
    pub sort_open: bool,
    pub replies: HashMap<usize, Replies>,
    pub related_selected: Option<usize>,
    pub related_pending: Option<usize>,
    pub related_failed: Option<usize>,
    pub comments_state: CommentsState,
    /// Bumped for each comments request, so a stale answer is dropped.
    pub comments_req: u64,
    /// LoadMoreComments arrived before the watch info (and its comments cursor).
    pub want_comments: bool,
    pub desc_expanded: bool,
    pub expanded_comments: HashSet<usize>,
    pub subscribed: bool,
    pub saved: bool,
    /// Where to seek once the video plays again (Back, or a remembered position).
    pub resume_at: Option<f64>,
}

#[derive(Default)]
pub struct SearchView {
    pub filter: SearchFilter,
    pub notices: Vec<SearchNotice>,
    pub chips: Vec<SearchOption>,
    pub filters: Vec<SearchFilterGroup>,
    pub rows: Vec<SearchRow>,
    pub transcripts: Vec<usize>,
    pub transcript_next: Option<Continuation>,
}

pub enum SearchRow {
    Video(usize),
    Channel(SearchChannel),
    Playlist(PlaylistRef),
    Shelf(String),
}

impl SearchView {
    pub fn update(&mut self, feed: &mut Feed, page: SearchPage) {
        self.notices = page.notices;
        if !page.chips.is_empty() {
            self.chips = page.chips;
        } else {
            for chip in &mut self.chips {
                chip.selected = chip.filter.as_ref() == Some(&self.filter);
            }
        }
        if !page.filters.is_empty() {
            self.filters = page.filters;
        }
        self.rows.clear();
        self.transcripts.clear();
        feed.items.clear();
        self.extend(feed, page.entries, page.results.items);
        for video in page.transcripts.items {
            self.transcripts.push(feed.items.len());
            feed.items.push(video);
        }
        feed.next = page.results.next;
        self.transcript_next = page.transcripts.next;
    }

    pub fn extend(&mut self, feed: &mut Feed, entries: Vec<SearchResult>, flat: Vec<Video>) {
        if entries.is_empty() {
            for video in flat {
                self.add_video(feed, video);
            }
        } else {
            for entry in entries {
                self.append(feed, entry);
            }
        }
    }

    fn append(&mut self, feed: &mut Feed, entry: SearchResult) {
        match entry {
            SearchResult::Video(v) => self.add_video(feed, *v),
            SearchResult::Channel(c) => self.rows.push(SearchRow::Channel(c)),
            SearchResult::Playlist(p) => self.rows.push(SearchRow::Playlist(p)),
            SearchResult::Shelf(s) => {
                self.rows.push(SearchRow::Shelf(s.title));
                for item in s.items {
                    self.append(feed, item);
                }
            }
        }
    }

    fn add_video(&mut self, feed: &mut Feed, v: Video) {
        self.rows.push(SearchRow::Video(feed.items.len()));
        feed.items.push(v);
    }
}

pub enum Kind {
    Home {
        topic: usize,
    },
    Search {
        query: String,
        view: Box<SearchView>,
    },
    Watch(Box<WatchState>),
    Channel {
        id: String,
        info: Option<Box<ChannelInfo>>,
        subscribed: bool,
        tab: ChannelTab,
        sort: Option<usize>,
    },
    Subscriptions {
        channels: Vec<ChannelRef>,
    },
    History,
    WatchLater,
}

/// Measured line counts, reused until the width bucket changes.
#[derive(Default)]
pub struct TextCache {
    titles_w: u32,
    titles: Vec<u8>,
    comments_w: u32,
    comments: Vec<u16>,
}

impl TextCache {
    pub fn clear_comments(&mut self) {
        self.comments.clear();
    }

    pub fn clear_titles(&mut self) {
        self.titles.clear();
    }
}

pub struct Page {
    pub id: PageId,
    pub kind: Kind,
    pub status: Load,
    pub resume: Option<(Video, f64)>,
    pub home_chips: Vec<Chip>,
    /// The page's main video list: home/search/channel/subscription grid, library list, or the
    /// related list on a watch page.
    pub feed: Feed,
    pub scroll: ScrollAnimator,
    /// Bumped whenever something that affects layout changes.
    pub version: u64,
    /// Bumped for every (re)load and "more" request, so stale answers are dropped.
    pub req: u64,
    pub layout: Option<(LayoutKey, Layout)>,
    pub text: TextCache,
}

impl Page {
    pub fn new(id: PageId, kind: Kind) -> Page {
        Page {
            id,
            kind,
            status: Load::Loading,
            resume: None,
            home_chips: Vec::new(),
            feed: Feed::default(),
            scroll: ScrollAnimator::new(),
            version: 0,
            req: 0,
            layout: None,
            text: TextCache::default(),
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
        self.version = self.version.wrapping_add(1);
    }

    pub fn is_home(&self) -> bool {
        matches!(self.kind, Kind::Home { .. })
    }

    pub fn is_watch(&self) -> bool {
        matches!(self.kind, Kind::Watch(_))
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
    /// The full guide (240 px) is open and pushes the content (never on Watch).
    pub guide: bool,
}

pub const TOP_BAR_H: f32 = 56.0;
pub const RAIL_W: f32 = 72.0;
pub const GUIDE_W: f32 = 240.0;
/// Below this the guide opens over the page instead of pushing it.
pub const GUIDE_PUSH_MIN: f32 = 1000.0;
pub const CHIPS_H: f32 = 56.0;

impl Geo {
    pub fn rail(&self, watch: bool) -> bool {
        !watch && !self.guide && self.win_w >= crate::theme::RAIL_MIN_WINDOW
    }
    pub fn content_x(&self, watch: bool) -> f32 {
        if !watch && self.guide {
            GUIDE_W
        } else if self.rail(watch) {
            RAIL_W
        } else {
            0.0
        }
    }
    pub fn content_w(&self, watch: bool) -> f32 {
        (self.win_w - self.content_x(watch)).max(100.0)
    }
    pub fn scroller_top(&self, home: bool) -> f32 {
        TOP_BAR_H + if home { CHIPS_H } else { 0.0 }
    }
    pub fn viewport_h(&self, home: bool) -> f32 {
        (self.win_h - self.scroller_top(home)).max(50.0)
    }
    /// Content padding: 24, 16 below 600.
    pub fn pad(&self) -> f32 {
        if self.win_w < 600.0 { 16.0 } else { 24.0 }
    }
}

pub const COL_GAP: f32 = 16.0;
pub const ROW_GAP: f32 = 40.0;
pub const AVATAR: f32 = 36.0;
/// Room the "⋮" button takes at the right of a card's text column.
pub const MENU_W: f32 = 24.0;

/// Columns = 1 below 600, 2 below 900, 3 below 1600, 4 below 2300, otherwise 5.
pub fn grid_cols(win_w: f32) -> usize {
    match win_w {
        w if w < 600.0 => 1,
        w if w < 900.0 => 2,
        w if w < 1600.0 => 3,
        w if w < 2300.0 => 4,
        _ => 5,
    }
}

/// Cards fill the row exactly.
pub fn card_w(inner: f32, cols: usize) -> f32 {
    let cols = cols.max(1) as f32;
    ((inner - COL_GAP * (cols - 1.0)) / cols).max(80.0)
}

/// Text column width of a card, bucketed to 8 px so measurement and drawing agree.
pub fn card_text_w(card_w: f32, avatar: bool) -> f32 {
    let w = card_w - MENU_W - if avatar { AVATAR + 12.0 } else { 0.0 };
    bucket8(w)
}

pub fn bucket8(w: f32) -> f32 {
    ((w.max(40.0) / 8.0).floor() * 8.0).max(40.0)
}

/// Search, History and Watch Later rows: content centered, max 1096 wide.
pub fn list_geometry(content_w: f32, pad: f32) -> (f32, f32) {
    let w = (content_w - 2.0 * pad).clamp(100.0, 1096.0);
    let x = ((content_w - w) / 2.0).max(pad.min(content_w / 2.0));
    (x, w)
}

pub fn list_thumb_w(list_w: f32) -> f32 {
    (list_w * 0.4).clamp(240.0, 500.0).min(list_w * 0.6).floor()
}

/// The text column of a list row needs title 52 + meta 18 + channel row 48 + snippet 18.
pub const LIST_TEXT_H: f32 = 136.0;
pub const LIST_GAP: f32 = 16.0;

pub const COMPACT_THUMB_W: f32 = 168.0;
pub const COMPACT_THUMB_H: f32 = 94.0;
pub const COMPACT_GAP: f32 = 8.0;
pub const SPINNER_H: f32 = 72.0;
pub const EMPTY_H: f32 = 320.0;
pub const ERROR_H: f32 = 240.0;
pub const RETRY_H: f32 = 56.0;

/// The watch page's columns and player box, in content coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WatchGeo {
    pub wide: bool,
    pub px: f32,
    pub pw: f32,
    pub sx: f32,
    pub sw: f32,
    /// Player box: x, y, w, h.
    pub player: (f32, f32, f32, f32),
    /// Where the columns start below the player.
    pub below: f32,
    /// Where the secondary column starts (wide layout).
    pub side_top: f32,
}

pub fn watch_geo(geo: &Geo, theatre: bool) -> WatchGeo {
    let w = geo.win_w;
    let h = geo.win_h;
    let pad = if w < 600.0 { 16.0 } else { 24.0 };
    let pmax = ((h - 169.0) * 16.0 / 9.0).max(320.0);
    let wide = w >= 1017.0;
    let (px, pw, sx, sw) = if wide {
        let avail = w - 2.0 * pad - 24.0;
        let sw = (avail * 0.29).clamp(300.0, 402.0);
        let pw = (avail - sw).min(pmax).max(200.0);
        let total = pw + 24.0 + sw;
        let left = ((w - total) / 2.0).max(pad);
        (left, pw, left + pw + 24.0, sw)
    } else {
        let pw = (w - 2.0 * pad).min(pmax).max(100.0);
        let left = ((w - pw) / 2.0).max(pad.min(w / 2.0));
        (left, pw, left, pw)
    };
    if theatre {
        let th = (w * 0.5625).min(h - 169.0).max(120.0).floor();
        WatchGeo {
            wide,
            px,
            pw,
            sx,
            sw,
            player: (0.0, 0.0, w, th),
            below: th + 24.0,
            side_top: th + 24.0,
        }
    } else {
        let ph = (pw * 9.0 / 16.0).floor();
        WatchGeo {
            wide,
            px,
            pw,
            sx,
            sw,
            player: (px, pad, pw, ph),
            below: pad + ph,
            side_top: pad,
        }
    }
}

/// The channel header's text column, next to the avatar.
pub fn channel_text_w(w: f32, avatar: f32) -> f32 {
    bucket8(w - avatar - 24.0)
}

/// Channel content: centered, max 1284.
pub fn channel_geometry(content_w: f32, pad: f32) -> (f32, f32) {
    let w = (content_w - 2.0 * pad).clamp(100.0, 1284.0);
    ((content_w - w) / 2.0, w)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Empty {
    NoSubscriptions,
    History,
    WatchLater,
    NoResults,
    NoVideos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    GridRow {
        first: usize,
        count: usize,
        card_w: f32,
        avatar: bool,
    },
    SkeletonRow {
        cols: usize,
        card_w: f32,
    },
    SkeletonList,
    ListRow(usize),
    CompactRow(usize),
    Spinner,
    MoreError,
    LoadError,
    Empty(Empty),
    /// One of the search page's notices.
    Notice(usize),
    SearchChips,
    SearchEntry(usize),
    /// "Matches in video transcripts · search.pronsh.dev".
    SectionLabel,
    PageTitle,
    ContinueWatching,
    ChannelBanner,
    ChannelHeader,
    ChannelTabs,
    ChannelSort,
    ChannelPlaylist(usize),
    SubsStrip,
    Player,
    WatchMeta {
        title_h: f32,
    },
    WatchDesc {
        lines: usize,
        shown: usize,
    },
    Chapter(usize),
    RelatedChips,
    PlaylistHeader,
    PlaylistVideo(usize),
    PlaylistMore,
    CommentsHeader,
    Reply {
        parent: usize,
        index: usize,
        lines: usize,
    },
    ReplyTail(usize),
    Comment {
        i: usize,
        lines: usize,
        shown: usize,
    },
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
    pub theatre: bool,
}

#[derive(Default, Debug)]
pub struct Layout {
    /// One or two columns of blocks, each sorted by `y`.
    pub columns: Vec<Vec<Block>>,
    pub content_h: f32,
    /// Watch page: where the comments section starts and the primary column ends.
    pub comments_y: Option<f32>,
    pub comments_end: f32,
    pub playlist_end: Option<f32>,
    /// Where the main video list ends (infinite scroll triggers near it).
    pub main_end: f32,
    /// Watch page: the player box in content coordinates.
    pub player: Option<(f32, f32, f32, f32)>,
}

impl Layout {
    /// Blocks of every column that intersect `[top, bottom)`.
    pub fn visible(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Block> {
        self.columns.iter().flat_map(move |col| {
            let start = col.partition_point(|b| b.y + b.h <= top);
            col.get(start..)
                .unwrap_or_default()
                .iter()
                .take_while(move |b| b.y < bottom)
        })
    }
}

/// Measures how many lines `text` wraps to at `size`/`weight` within `width`.
pub trait Measure {
    fn lines(&mut self, text: &str, size: f32, weight: u16, width: f32) -> usize;
}

pub const BODY_LH: f32 = 20.0;
pub const DESC_CLAMP: usize = 3;
pub const COMMENT_CLAMP: usize = 4;

/// Comment text column: avatar 40 + 16 gap.
pub fn comment_text_w(pw: f32) -> f32 {
    ((pw - 56.0).max(60.0) / 16.0).floor() * 16.0
}

pub fn comment_h(shown: usize, clamped: bool) -> f32 {
    let text = shown.max(1) as f32 * BODY_LH;
    let more = if clamped { 24.0 } else { 0.0 };
    (18.0 + 2.0 + text + more + 4.0 + 32.0).max(40.0) + 16.0
}

/// Title lines of every card in the feed, measured at `text_w`, cached on the page.
fn title_lines(page: &mut Page, text_w: f32, m: &mut dyn Measure) {
    let key = text_w as u32;
    if page.text.titles_w != key {
        page.text.titles.clear();
        page.text.titles_w = key;
    }
    let start = page.text.titles.len().min(page.feed.items.len());
    for v in page.feed.items.get(start..).unwrap_or_default() {
        let n = m.lines(&v.title, 16.0, 500, text_w).clamp(1, 2);
        page.text.titles.push(n as u8);
    }
}

struct GridGeometry {
    x: f32,
    w: f32,
    y: f32,
}

fn grid_section(
    page: &mut Page,
    geo: &Geo,
    geometry: GridGeometry,
    avatar: bool,
    col: &mut Vec<Block>,
    m: &mut dyn Measure,
) -> f32 {
    let GridGeometry { x, w, mut y } = geometry;
    let cols = grid_cols(geo.win_w);
    let cw = card_w(w, cols);
    let thumb_h = cw * 9.0 / 16.0;
    match &page.status {
        Load::Loading => {
            for _ in 0..3 {
                let h = thumb_h + 12.0 + 64.0;
                col.push(Block {
                    x,
                    y,
                    w,
                    h,
                    kind: BlockKind::SkeletonRow { cols, card_w: cw },
                });
                y += h + ROW_GAP;
            }
        }
        Load::Failed(_) => {
            col.push(Block {
                x,
                y,
                w,
                h: ERROR_H,
                kind: BlockKind::LoadError,
            });
            y += ERROR_H;
        }
        Load::Ready if page.feed.items.is_empty() => {
            col.push(Block {
                x,
                y,
                w,
                h: EMPTY_H,
                kind: BlockKind::Empty(Empty::NoVideos),
            });
            y += EMPTY_H;
        }
        Load::Ready => {
            let tw = card_text_w(cw, avatar);
            title_lines(page, tw, m);
            let n = page.feed.items.len();
            let mut first = 0;
            while first < n {
                let count = cols.min(n - first);
                let max_lines = page
                    .text
                    .titles
                    .get(first..first + count)
                    .and_then(|s| s.iter().max().copied())
                    .unwrap_or(2) as f32;
                let text_h = max_lines * 22.0 + if avatar { 40.0 } else { 20.0 };
                let h = thumb_h + 12.0 + text_h.max(if avatar { AVATAR } else { 0.0 });
                col.push(Block {
                    x,
                    y,
                    w,
                    h,
                    kind: BlockKind::GridRow {
                        first,
                        count,
                        card_w: cw,
                        avatar,
                    },
                });
                y += h + ROW_GAP;
                first += count;
            }
        }
    }
    feed_tail(&page.feed, x, w, y, col)
}

fn feed_tail(feed: &Feed, x: f32, w: f32, mut y: f32, col: &mut Vec<Block>) -> f32 {
    if feed.loading_more {
        col.push(Block {
            x,
            y,
            w,
            h: SPINNER_H,
            kind: BlockKind::Spinner,
        });
        y += SPINNER_H;
    } else if feed.more_error.is_some() {
        col.push(Block {
            x,
            y,
            w,
            h: RETRY_H,
            kind: BlockKind::MoreError,
        });
        y += RETRY_H;
    }
    y
}

pub fn build_layout(page: &mut Page, geo: &Geo, theatre: bool, m: &mut dyn Measure) -> Layout {
    let pad = geo.pad();
    let watch = page.is_watch();
    let content_w = geo.content_w(watch);
    let mut lay = Layout {
        columns: vec![Vec::new()],
        ..Default::default()
    };
    let mut col: Vec<Block> = Vec::new();
    let mut y = if page.is_home() { 0.0 } else { pad.min(16.0) };
    match &page.kind {
        Kind::Watch(_) => return watch_layout(page, geo, theatre, m),
        Kind::Home { .. } => {
            let inner = content_w - 2.0 * pad;
            if page.resume.is_some() {
                col.push(Block {
                    x: pad,
                    y,
                    w: inner,
                    h: 104.0,
                    kind: BlockKind::ContinueWatching,
                });
                y += 120.0;
            }
            y = grid_section(
                page,
                geo,
                GridGeometry {
                    x: pad,
                    w: inner,
                    y: y + 8.0,
                },
                true,
                &mut col,
                m,
            );
        }
        Kind::Channel { info, tab, .. } => {
            let (x, w) = channel_geometry(content_w, pad);
            if let Some(info) = info {
                if info.banner.is_some() {
                    let bh = (w / 6.2).floor();
                    col.push(Block {
                        x,
                        y,
                        w,
                        h: bh,
                        kind: BlockKind::ChannelBanner,
                    });
                    y += bh + 16.0;
                }
                let hh = 192.0;
                col.push(Block {
                    x,
                    y,
                    w,
                    h: hh,
                    kind: BlockKind::ChannelHeader,
                });
                y += hh;
                col.push(Block {
                    x,
                    y,
                    w,
                    h: 48.0,
                    kind: BlockKind::ChannelTabs,
                });
                y += 48.0 + 18.0;
                if *tab == ChannelTab::Videos && !info.video_sort.is_empty() {
                    col.push(Block {
                        x,
                        y,
                        w,
                        h: 44.0,
                        kind: BlockKind::ChannelSort,
                    });
                    y += 50.0;
                }
                if *tab == ChannelTab::Playlists
                    && page.status == Load::Ready
                    && !info.playlists.items.is_empty()
                {
                    let (lx, lw) = list_geometry(content_w, pad);
                    let h = (list_thumb_w(lw) * 9.0 / 16.0).max(136.0);
                    for i in 0..info.playlists.items.len() {
                        col.push(Block {
                            x: lx,
                            y,
                            w: lw,
                            h,
                            kind: BlockKind::ChannelPlaylist(i),
                        });
                        y += h + LIST_GAP;
                    }
                    y = feed_tail(&page.feed, lx, lw, y, &mut col);
                    lay.main_end = y;
                    lay.content_h = y + pad;
                    lay.columns = vec![col];
                    return lay;
                }
            }
            y = grid_section(page, geo, GridGeometry { x, w, y }, false, &mut col, m);
        }
        Kind::Subscriptions { channels } => {
            let inner = content_w - 2.0 * pad;
            if page.status == Load::Ready && channels.is_empty() {
                col.push(Block {
                    x: pad,
                    y,
                    w: inner,
                    h: EMPTY_H,
                    kind: BlockKind::Empty(Empty::NoSubscriptions),
                });
                y += EMPTY_H;
            } else {
                if !channels.is_empty() {
                    col.push(Block {
                        x: pad,
                        y,
                        w: inner,
                        h: 120.0,
                        kind: BlockKind::SubsStrip,
                    });
                    y += 120.0 + 16.0;
                }
                y = grid_section(
                    page,
                    geo,
                    GridGeometry {
                        x: pad,
                        w: inner,
                        y,
                    },
                    true,
                    &mut col,
                    m,
                );
            }
        }
        Kind::Search { .. } | Kind::History | Kind::WatchLater => {
            let (x, w) = list_geometry(content_w, pad);
            let library = !matches!(page.kind, Kind::Search { .. });
            if library {
                col.push(Block {
                    x,
                    y,
                    w,
                    h: 56.0,
                    kind: BlockKind::PageTitle,
                });
                y += 56.0 + 8.0;
            }
            let empty = match page.kind {
                Kind::History => Empty::History,
                Kind::WatchLater => Empty::WatchLater,
                _ => Empty::NoResults,
            };
            let th = (list_thumb_w(w) * 9.0 / 16.0).round();
            let row_h = th.max(LIST_TEXT_H);
            let search = match &page.kind {
                Kind::Search { view, .. } => Some(view.as_ref()),
                _ => None,
            };
            if search.is_some_and(|s| !s.chips.is_empty() || !s.filters.is_empty()) {
                col.push(Block {
                    x,
                    y,
                    w,
                    h: 40.0,
                    kind: BlockKind::SearchChips,
                });
                y += 52.0;
            }
            let notices = search
                .filter(|_| page.status == Load::Ready)
                .map_or(&[][..], |s| s.notices.as_slice());
            let mut shown_notice = false;
            for (i, n) in notices.iter().enumerate() {
                let h = match n {
                    SearchNotice::AgeGated { body, .. } => {
                        let lines = m.lines(body, 14.0, 400, bucket8(w - 32.0)).max(1);
                        16.0 + 28.0 + 4.0 + lines as f32 * BODY_LH + 16.0
                    }
                    SearchNotice::Corrected { .. } => 56.0,
                    SearchNotice::DidYouMean { .. } => 32.0,
                    // Drawn as the empty state, only when nothing else shows.
                    SearchNotice::NoResults => continue,
                };
                col.push(Block {
                    x,
                    y,
                    w,
                    h,
                    kind: BlockKind::Notice(i),
                });
                y += h + 16.0;
                shown_notice = true;
            }
            match &page.status {
                Load::Loading => {
                    for _ in 0..4 {
                        col.push(Block {
                            x,
                            y,
                            w,
                            h: row_h,
                            kind: BlockKind::SkeletonList,
                        });
                        y += row_h + LIST_GAP;
                    }
                }
                Load::Failed(_) => {
                    col.push(Block {
                        x,
                        y,
                        w,
                        h: ERROR_H,
                        kind: BlockKind::LoadError,
                    });
                    y += ERROR_H;
                }
                Load::Ready
                    if page.feed.items.is_empty() && search.is_none_or(|s| s.rows.is_empty()) =>
                {
                    // An age-gate or a correction already says why; otherwise "No results".
                    if !shown_notice {
                        col.push(Block {
                            x,
                            y,
                            w,
                            h: EMPTY_H,
                            kind: BlockKind::Empty(empty),
                        });
                        y += EMPTY_H;
                    }
                }
                Load::Ready => {
                    if let Some(search) = search {
                        for (i, result) in search.rows.iter().enumerate() {
                            let (kind, h) = match result {
                                SearchRow::Video(index) => {
                                    (BlockKind::ListRow(*index), row_h.max(LIST_TEXT_H + 16.0))
                                }
                                SearchRow::Shelf(_) => (BlockKind::SearchEntry(i), 40.0),
                                SearchRow::Channel(_) => (BlockKind::SearchEntry(i), 152.0),
                                SearchRow::Playlist(_) => (BlockKind::SearchEntry(i), row_h),
                            };
                            col.push(Block { x, y, w, h, kind });
                            y += h + LIST_GAP;
                        }
                        if !search.transcripts.is_empty() {
                            col.push(Block {
                                x,
                                y,
                                w,
                                h: 40.0,
                                kind: BlockKind::SectionLabel,
                            });
                            y += 48.0;
                        }
                        for i in &search.transcripts {
                            let h = row_h.max(LIST_TEXT_H + 50.0);
                            col.push(Block {
                                x,
                                y,
                                w,
                                h,
                                kind: BlockKind::ListRow(*i),
                            });
                            y += h + LIST_GAP;
                        }
                    } else {
                        for i in 0..page.feed.items.len() {
                            col.push(Block {
                                x,
                                y,
                                w,
                                h: row_h,
                                kind: BlockKind::ListRow(i),
                            });
                            y += row_h + LIST_GAP;
                        }
                    }
                }
            }
            y = feed_tail(&page.feed, x, w, y, &mut col);
        }
    }
    lay.columns = vec![col];
    lay.content_h = y + pad;
    lay.main_end = y;
    lay
}

fn watch_layout(page: &mut Page, geo: &Geo, theatre: bool, m: &mut dyn Measure) -> Layout {
    let wg = watch_geo(geo, theatre);
    let mut lay = Layout {
        player: Some(wg.player),
        ..Default::default()
    };
    let (x, pw) = (wg.px, wg.pw);
    let mut col: Vec<Block> = Vec::new();
    let (px_, py, pww, ph) = wg.player;
    col.push(Block {
        x: px_,
        y: py,
        w: pww,
        h: ph,
        kind: BlockKind::Player,
    });
    let mut y = wg.below;
    let mut comments_at: Option<usize> = None;
    let status = page.status.clone();
    let related_n = page.feed.items.len();
    let Kind::Watch(w) = &mut page.kind else {
        return lay;
    };
    match (&status, &w.info) {
        (Load::Failed(_), _) => {
            col.push(Block {
                x,
                y: y + 16.0,
                w: pw,
                h: ERROR_H,
                kind: BlockKind::LoadError,
            });
            y += 16.0 + ERROR_H;
        }
        (_, None) => {
            col.push(Block {
                x,
                y,
                w: pw,
                h: SPINNER_H,
                kind: BlockKind::Spinner,
            });
            y += SPINNER_H;
        }
        (_, Some(info)) => {
            let title_lines = m.lines(&info.title, 20.0, 700, bucket8(pw)).max(1);
            let title_h = title_lines as f32 * 28.0;
            let meta_h = 12.0 + title_h + 12.0 + 40.0;
            col.push(Block {
                x,
                y,
                w: pw,
                h: meta_h,
                kind: BlockKind::WatchMeta { title_h },
            });
            y += meta_h + 12.0;
            let desc = info.description.trim();
            let lines = if desc.is_empty() {
                0
            } else {
                m.lines(desc, 14.0, 400, bucket8(pw - 24.0))
            };
            let shown = if w.desc_expanded {
                lines
            } else {
                lines.min(DESC_CLAMP)
            };
            let toggle = if w.desc_expanded && (lines > DESC_CLAMP || !info.chapters.is_empty()) {
                28.0
            } else {
                0.0
            };
            let desc_h = 12.0 + 20.0 + shown as f32 * BODY_LH + toggle + 12.0;
            col.push(Block {
                x,
                y,
                w: pw,
                h: desc_h,
                kind: BlockKind::WatchDesc { lines, shown },
            });
            y += desc_h;
            if w.desc_expanded {
                for i in 0..info.chapters.len() {
                    col.push(Block {
                        x,
                        y,
                        w: pw,
                        h: 60.0,
                        kind: BlockKind::Chapter(i),
                    });
                    y += 60.0;
                }
            }
            if info.comments.is_some() || !w.comments.is_empty() {
                comments_at = Some(col.len());
            }
        }
    }
    // Comments, measured at a 16 px bucketed width (a live resize re-measures every 16 px).
    let mut comment_blocks: Vec<Block> = Vec::new();
    let mut cy = 0.0;
    if comments_at.is_some() {
        cy += 24.0;
        let header_h = 52.0 + if w.sort_open { 40.0 } else { 0.0 };
        comment_blocks.push(Block {
            x,
            y: cy,
            w: pw,
            h: header_h,
            kind: BlockKind::CommentsHeader,
        });
        cy += header_h;
        let tw = comment_text_w(pw);
        let key = tw as u32;
        if page.text.comments_w != key {
            page.text.comments.clear();
            page.text.comments_w = key;
        }
        let start = page.text.comments.len().min(w.comments.len());
        for c in w.comments.get(start..).unwrap_or_default() {
            let n = m.lines(c.text.trim_end(), 14.0, 400, tw).max(1);
            page.text.comments.push(n.min(u16::MAX as usize) as u16);
        }
        for (i, n) in page.text.comments.iter().enumerate() {
            let lines = *n as usize;
            let expanded = w.expanded_comments.contains(&i);
            let shown = if expanded {
                lines
            } else {
                lines.min(COMMENT_CLAMP)
            };
            let Some(comment) = w.comments.get(i) else {
                continue;
            };
            let h = comment_h(shown, lines > COMMENT_CLAMP)
                + if comment.pinned { 22.0 } else { 0.0 }
                + if comment.replies.is_some() { 36.0 } else { 0.0 };
            comment_blocks.push(Block {
                x,
                y: cy,
                w: pw,
                h,
                kind: BlockKind::Comment { i, lines, shown },
            });
            cy += h;
            if let Some(replies) = w.replies.get(&i).filter(|r| r.expanded) {
                let (x, pw) = (x + 56.0, (pw - 56.0).max(100.0));
                for (index, reply) in replies.items.iter().enumerate() {
                    let lines = m
                        .lines(reply.text.trim_end(), 14.0, 400, comment_text_w(pw))
                        .max(1);
                    let h = comment_h(lines, false);
                    comment_blocks.push(Block {
                        x,
                        y: cy,
                        w: pw,
                        h,
                        kind: BlockKind::Reply {
                            parent: i,
                            index,
                            lines,
                        },
                    });
                    cy += h;
                }
                if replies.next.is_some() || replies.status == CommentsState::Loading {
                    comment_blocks.push(Block {
                        x,
                        y: cy,
                        w: pw,
                        h: 44.0,
                        kind: BlockKind::ReplyTail(i),
                    });
                    cy += 44.0;
                }
            }
        }
        match w.comments_state {
            CommentsState::Loading => {
                comment_blocks.push(Block {
                    x,
                    y: cy,
                    w: pw,
                    h: SPINNER_H,
                    kind: BlockKind::CommentsSpinner,
                });
                cy += SPINNER_H;
            }
            CommentsState::Failed(_) => {
                comment_blocks.push(Block {
                    x,
                    y: cy,
                    w: pw,
                    h: RETRY_H,
                    kind: BlockKind::CommentsError,
                });
                cy += RETRY_H;
            }
            _ => {}
        }
    }
    // Related videos: their own column when wide; below 1017 after the description and before
    // the comments.
    let mut related: Vec<Block> = Vec::new();
    let (rx, rw) = if wg.wide { (wg.sx, wg.sw) } else { (x, pw) };
    let mut ry = if wg.wide { wg.side_top } else { y + 24.0 };
    if let Some(panel) = w.info.as_ref().and_then(|i| i.playlist.as_ref()) {
        related.push(Block {
            x: rx,
            y: ry,
            w: rw,
            h: 44.0,
            kind: BlockKind::PlaylistHeader,
        });
        ry += 44.0;
        for i in 0..panel.items.len() {
            related.push(Block {
                x: rx,
                y: ry,
                w: rw,
                h: 70.0,
                kind: BlockKind::PlaylistVideo(i),
            });
            ry += 72.0;
        }
        if panel.next.is_some() {
            related.push(Block {
                x: rx,
                y: ry,
                w: rw,
                h: RETRY_H,
                kind: BlockKind::PlaylistMore,
            });
            ry += RETRY_H;
            lay.playlist_end = Some(ry);
        }
        ry += 16.0;
    }
    if related_n > 0 && w.info.as_ref().is_some_and(|i| !i.related_chips.is_empty()) {
        related.push(Block {
            x: rx,
            y: ry,
            w: rw,
            h: 48.0,
            kind: BlockKind::RelatedChips,
        });
        ry += 48.0;
    }
    for i in 0..related_n {
        related.push(Block {
            x: rx,
            y: ry,
            w: rw,
            h: COMPACT_THUMB_H,
            kind: BlockKind::CompactRow(i),
        });
        ry += COMPACT_THUMB_H + COMPACT_GAP;
    }
    if page.feed.loading_more {
        related.push(Block {
            x: rx,
            y: ry,
            w: rw,
            h: SPINNER_H,
            kind: BlockKind::Spinner,
        });
        ry += SPINNER_H;
    } else if page.feed.more_error.is_some() {
        related.push(Block {
            x: rx,
            y: ry,
            w: rw,
            h: RETRY_H,
            kind: BlockKind::MoreError,
        });
        ry += ERROR_H;
    }
    if wg.wide {
        lay.main_end = ry;
        let base = y;
        if comments_at.is_some() {
            lay.comments_y = Some(base + 24.0);
        }
        for mut b in comment_blocks {
            b.y += base;
            col.push(b);
        }
        y = base + cy;
        lay.comments_end = y;
        lay.columns = vec![col, related];
        lay.content_h = y.max(ry) + 24.0;
    } else {
        col.extend(related);
        let base = if related_n > 0 { ry } else { y };
        lay.main_end = base;
        if comments_at.is_some() {
            lay.comments_y = Some(base + 24.0);
        }
        for mut b in comment_blocks {
            b.y += base;
            col.push(b);
        }
        y = base + cy;
        lay.comments_end = y;
        lay.columns = vec![col];
        lay.content_h = y + 24.0;
    }
    lay
}
