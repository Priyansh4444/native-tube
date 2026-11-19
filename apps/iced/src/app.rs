//! Application state, messages and `update`.
//!
//! State is a navigation stack of pages; each page owns its data and its `ScrollModel`, so a
//! page under the top keeps everything and Back restores it as it was. Forward re-pushes what
//! Back popped. Both stacks are bounded.
//!
//! Network work is a `nt_core::Job` turned into an iced `Task`; every answer carries the page id
//! and a request token for what asked (topic, "more" page, comments page), so stale answers are
//! dropped. Errors are logged raw and shown as a plain sentence with a Retry that redoes only
//! what failed.

use crate::ext::{self, Ext};
use crate::icons::Icon;
use crate::imgcache::Images;
use crate::prefs::Prefs;
use crate::scroller::{ScrollModel, mark_on_next_frame};
use iced::advanced::image::Allocation;
use iced::keyboard::{self, key::Named};
use iced::widget::image::Handle as ImageHandle;
use iced::{Point, Size, Subscription, Task, event, mouse, window};
use nt_core::autopilot::Command;
use nt_core::metrics::marks;
use nt_core::{
    ChannelInfo, ChannelRef, Comment, CommentPage, Continuation, PlaybackState, Status, TOPICS,
    Video, VideoFrame, WatchInfo, client, library,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The search field's widget id (for the `/` shortcut).
pub const SEARCH_ID: &str = "search";
/// Pages kept behind the current one; the oldest (after the root) go first.
const MAX_PAGES: usize = 24;
const MAX_FORWARD: usize = 8;
pub const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Home,
    Subscriptions,
    History,
    WatchLater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRef {
    Main,
    Related,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuPage {
    Main,
    Speed,
    Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Seek(f64),
    Search(String),
    Url(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Nav(Root),
    Back,
    Forward,
    Quit,
    Pilot(Command),
    Ext(Ext),
    Allocated(ImageHandle, Result<Allocation, iced_runtime::image::Error>),
    ScrollMoved,
    Topic(usize),
    /// List, index, and a hash of the video id seen when the row was built.
    Open(ListRef, usize, u64),
    /// Mouse pressed on a card: start loading its video before the click completes.
    Preload(ListRef, usize, u64),
    PreloadExpire(u64),
    OpenChannelOf(ListRef, usize),
    OpenChannel(ChannelRef),
    OpenWatchChannel,
    FeedLoaded {
        page: u64,
        req: u64,
        append: bool,
        result: nt_core::Result<nt_core::Page<Video>>,
    },
    WatchLoaded(u64, nt_core::Result<WatchInfo>),
    CommentsLoaded(u64, u64, nt_core::Result<CommentPage>),
    ChannelLoaded(u64, nt_core::Result<ChannelInfo>),
    SubsLoaded(u64, nt_core::Result<Vec<Video>>),
    Retry,
    RetryMore,
    RetryComments,
    LoadComments,
    SearchInput(String),
    SearchSubmit,
    SearchFocus(bool),
    SearchOpen(bool),
    SuggestNav(i32),
    SuggestClose,
    SuggestFire(u64),
    Suggestions(u64, nt_core::Result<Vec<String>>),
    SuggestPick(usize),
    FocusSearch,
    ToggleSubscribe,
    ToggleLater,
    Share,
    ClearHistory,
    ToggleDesc,
    ToggleComment(usize),
    DescLink(Link),
    CardMenu(ListRef, usize),
    /// Right-click on a card: the same menu, at the pointer.
    CardContext(ListRef, usize),
    CardMenuClose,
    CardNotInterested,
    CardOpenChannel,
    CardSaveLater,
    CardCopyLink,
    Toast(String),
    ToastDone(u64),
    // Player.
    PlayPause,
    ClickVideo,
    SeekTo(f64),
    SeekFrac(f32),
    SeekBy(f64),
    SeekPercent(u8),
    FrameStep(i32),
    SpeedStep(i32),
    SetVolume(f32),
    VolumeBy(f64),
    ToggleMute,
    VolumeHover(bool),
    Menu(Option<MenuPage>),
    SetSpeed(f64),
    SetQuality(Option<u32>),
    ToggleAutoplay,
    ToggleFullscreen,
    ModeIs(window::Mode),
    ToggleTheatre,
    Escape,
    RetryPlayback,
    UpNextTick(u64),
    UpNextCancel,
    UpNextPlay,
    Replay,
    WindowSize(Size),
    WindowId(Option<window::Id>),
    Scale(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Load {
    Loading,
    Failed(String),
    Ready,
}

#[derive(Debug, Clone, PartialEq)]
pub enum More {
    Idle,
    Loading,
    Failed(String),
}

#[derive(Default)]
pub struct Feed {
    pub items: Vec<Video>,
    ids: HashSet<String>,
    pub next: Option<Continuation>,
    pub more: Option<More>,
}

impl Feed {
    fn set(&mut self, page: nt_core::Page<Video>) {
        self.items.clear();
        self.ids.clear();
        self.next = None;
        self.append(page);
    }
    fn append(&mut self, page: nt_core::Page<Video>) {
        for v in page.items {
            if self.ids.insert(v.id.clone()) {
                self.items.push(v);
            }
        }
        self.next = page.next;
        self.more = Some(More::Idle);
    }
}

pub struct Home {
    pub topic: usize,
    pub load: Load,
    pub feed: Feed,
    pub req: u64,
    pub chip_offset: Cell<f32>,
}

pub struct Search {
    pub query: String,
    pub load: Load,
    pub feed: Feed,
}

pub struct Channel {
    pub id: String,
    pub stub: ChannelRef,
    pub load: Load,
    pub info: Option<ChannelInfo>,
    pub feed: Feed,
    pub desc_open: bool,
}

pub struct Subs {
    pub channels: Vec<ChannelRef>,
    pub load: Load,
    pub feed: Feed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Comments {
    NotStarted,
    Loading,
    Idle,
    Failed(String),
}

/// Prefix sums over a variable-height list (comments), so a view finds the visible items by
/// binary search instead of walking every loaded item.
#[derive(Default)]
pub struct VarList {
    pub version: u64,
    pub width: f32,
    pub prefix: Vec<f32>,
}

pub struct Watch {
    pub video: Video,
    pub load: Load,
    pub info: Option<WatchInfo>,
    pub related: Vec<Video>,
    pub comments: Vec<Comment>,
    /// Character counts, for height estimates before a comment has been laid out.
    pub comment_chars: Vec<u32>,
    pub comments_next: Option<Continuation>,
    pub comments_total: Option<String>,
    pub comments_state: Comments,
    pub comments_req: u64,
    pub comments_open: HashSet<usize>,
    pub comments_layout: RefCell<VarList>,
    pub desc_open: bool,
    /// The description split into plain runs and links, parsed once when it arrives.
    pub desc_spans: Vec<(std::ops::Range<usize>, Option<Link>)>,
    pub resume: f64,
    /// Where the comments header sits in the content, as of the last build.
    pub comments_y: Cell<f32>,
}

// A handful of pages exist at a time; boxing the big variants would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum Body {
    Home(Home),
    Search(Search),
    Watch(Box<Watch>),
    Channel(Channel),
    Subs(Subs),
    History(Vec<Video>),
    Later(Vec<Video>),
}

pub struct Page {
    pub id: u64,
    pub scroll: ScrollModel,
    pub body: Body,
}

/// The countdown card shown when a video ends.
#[derive(Debug, Clone)]
pub struct UpNext {
    pub page: u64,
    pub remaining: u32,
    pub counting: bool,
    pub seq: u64,
}

#[derive(Debug, Clone)]
pub struct CardMenu {
    pub list: ListRef,
    pub index: usize,
    /// Hash of the video id the menu was opened for (the list may change under it).
    pub hash: u64,
    pub at: Point,
    /// Opened by a right-click (at the pointer) rather than by the "⋮" button.
    pub context: bool,
}

pub struct App {
    pub pages: Vec<Page>,
    forward: Vec<Page>,
    next_id: u64,
    pub win: Size,
    pub scale: f32,
    window: Option<window::Id>,
    pub images: Images,
    pub prefs: Prefs,
    // Search field and suggestions.
    pub query: String,
    pub suggestions: Vec<String>,
    /// The text the suggestions answer (for bolding the completion).
    pub sugg_for: String,
    pub sugg_sel: Option<usize>,
    pub sugg_open: bool,
    sugg_seq: u64,
    pub search_focused: bool,
    pub search_open: bool,
    pub blur: u64,
    // Playback.
    pub play: PlaybackState,
    pub frame: Option<Arc<VideoFrame>>,
    frame_gen: u64,
    pub playing_page: Option<u64>,
    resume_pending: Option<f64>,
    /// The seek issued for a resume; frames before it lands are not shown.
    seeking_to: Option<f64>,
    /// The last picture of the watch page we left, shown again while it reloads on Back.
    saved_frame: Option<(u64, Arc<VideoFrame>)>,
    want_playback_mark: bool,
    pub player_error: Option<String>,
    pub fullscreen: bool,
    pub menu: Option<MenuPage>,
    pub volume_hover: bool,
    pub bezel: Option<(Icon, Instant)>,
    pub up_next: Option<UpNext>,
    up_seq: u64,
    pub card_menu: Option<CardMenu>,
    pub toast: Option<(String, u64)>,
    toast_seq: u64,
    /// A video started on mouse press, waiting for the click that opens its page.
    preload: Option<(String, u64)>,
    preload_seq: u64,
}

/// A plain sentence for an error; the raw text goes to stderr.
pub fn friendly(e: &nt_core::Error) -> String {
    eprintln!("nt-iced: {e}");
    match e {
        nt_core::Error::Network(m) if m.contains("pausing") => {
            "YouTube asked us to slow down. Try again in a few minutes.".into()
        }
        nt_core::Error::Network(_) => "Couldn't reach YouTube. Check your connection.".into(),
        nt_core::Error::Parse(_) => "YouTube sent something this app doesn't understand.".into(),
        nt_core::Error::Fixture(_) => "This page isn't in the recorded test data.".into(),
        nt_core::Error::Player(_) => "This video can't be played.".into(),
        nt_core::Error::Cancelled => "The request was cancelled.".into(),
        nt_core::Error::Io(_) => "Couldn't read or write local data.".into(),
    }
}

pub fn id_hash(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// The cursor position in window coordinates, kept by the root widget (for anchoring menus).
pub fn last_cursor() -> Point {
    crate::scroller::LAST_CURSOR.with(|c| c.get())
}

impl App {
    pub fn boot() -> (App, Task<Message>) {
        crate::scroller::trace_event("boot");
        let mut app = App {
            pages: Vec::new(),
            forward: Vec::new(),
            next_id: 1,
            win: Size::new(1280.0, 800.0),
            scale: 1.0,
            window: None,
            images: Images::default(),
            prefs: Prefs::load(),
            query: String::new(),
            suggestions: Vec::new(),
            sugg_for: String::new(),
            sugg_sel: None,
            sugg_open: false,
            sugg_seq: 0,
            search_focused: false,
            search_open: false,
            blur: 0,
            play: PlaybackState::default(),
            frame: None,
            frame_gen: 0,
            playing_page: None,
            resume_pending: None,
            seeking_to: None,
            saved_frame: None,
            want_playback_mark: false,
            player_error: None,
            fullscreen: false,
            menu: None,
            volume_hover: false,
            bezel: None,
            up_next: None,
            up_seq: 0,
            card_menu: None,
            toast: None,
            toast_seq: 0,
            preload: None,
            preload_seq: 0,
        };
        let task = app.replace_root(Root::Home);
        mark_on_next_frame(marks::FIRST_FRAME);
        let scale = window::latest()
            .and_then(window::scale_factor)
            .map(Message::Scale);
        let id = window::latest().map(Message::WindowId);
        (app, Task::batch([task, scale, id]))
    }

    /// The page on top. The stack is never empty (every path that pops keeps the root).
    pub fn top(&self) -> Option<&Page> {
        self.pages.last()
    }
    fn top_mut(&mut self) -> Option<&mut Page> {
        self.pages.last_mut()
    }
    pub fn top_id(&self) -> u64 {
        self.top().map(|p| p.id).unwrap_or(0)
    }

    /// The root shown, if the root page is what's on screen (only then is a rail entry lit).
    pub fn shown_root(&self) -> Option<Root> {
        if self.pages.len() != 1 {
            return None;
        }
        match &self.pages.first()?.body {
            Body::Home(_) => Some(Root::Home),
            Body::Subs(_) => Some(Root::Subscriptions),
            Body::History(_) => Some(Root::History),
            Body::Later(_) => Some(Root::WatchLater),
            _ => None,
        }
    }

    fn new_page(&mut self, body: Body) -> Page {
        let id = self.next_id;
        self.next_id += 1;
        Page {
            id,
            scroll: ScrollModel::default(),
            body,
        }
    }

    fn page_mut(&mut self, id: u64) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    // ---- navigation ---------------------------------------------------------------------

    /// Leaving the page on top: stop its video and remember where it was.
    fn leave_top(&mut self) {
        self.menu = None;
        self.card_menu = None;
        self.up_next = None;
        self.volume_hover = false;
        let playing = self.playing_page;
        let Some(page) = self.pages.last_mut() else {
            return;
        };
        if let Body::Watch(w) = &mut page.body
            && playing == Some(page.id)
        {
            w.resume = self.play.position;
            if self.play.duration > 0.0 && !self.play.is_live {
                let at = if self.play.status == Status::Ended {
                    self.play.duration
                } else {
                    self.play.position
                };
                self.prefs.record(&w.video.id, at, self.play.duration);
                self.prefs.save();
            }
            if let Some(p) = ext::existing_player() {
                p.stop();
            }
            self.playing_page = None;
            self.saved_frame = self.frame.take().map(|f| (page.id, f));
            self.resume_pending = None;
            self.seeking_to = None;
            self.want_playback_mark = false;
        }
    }

    fn push(&mut self, body: Body) -> u64 {
        self.leave_top();
        self.close_suggestions();
        self.forward.clear();
        let page = self.new_page(body);
        let id = page.id;
        self.pages.push(page);
        if self.pages.len() > MAX_PAGES {
            self.pages.remove(1);
        }
        id
    }

    fn replace_root(&mut self, root: Root) -> Task<Message> {
        self.leave_top();
        self.close_suggestions();
        self.forward.clear();
        let body = match root {
            Root::Home => Body::Home(Home {
                topic: 0,
                load: Load::Loading,
                feed: Feed::default(),
                req: 0,
                chip_offset: Cell::new(0.0),
            }),
            Root::Subscriptions => {
                let channels = library().subscriptions();
                let load = if channels.is_empty() {
                    Load::Ready
                } else {
                    Load::Loading
                };
                Body::Subs(Subs {
                    channels,
                    load,
                    feed: Feed::default(),
                })
            }
            Root::History => Body::History(library().history()),
            Root::WatchLater => Body::Later(library().watch_later()),
        };
        let page = self.new_page(body);
        self.pages = vec![page];
        self.load_top()
    }

    fn back(&mut self) -> Task<Message> {
        if self.pages.len() < 2 {
            return Task::none();
        }
        self.leave_top();
        self.close_suggestions();
        if let Some(page) = self.pages.pop() {
            self.forward.push(page);
            if self.forward.len() > MAX_FORWARD {
                self.forward.remove(0);
            }
        }
        self.arrive();
        Task::none()
    }

    fn go_forward(&mut self) -> Task<Message> {
        let Some(page) = self.forward.pop() else {
            return Task::none();
        };
        self.leave_top();
        self.close_suggestions();
        self.pages.push(page);
        self.arrive();
        Task::none()
    }

    /// A page came back on top (Back or Forward): resume its video, refresh library lists.
    fn arrive(&mut self) {
        let Some(top) = self.top() else { return };
        let id = top.id;
        match &top.body {
            Body::Search(s) => self.query = s.query.clone(),
            Body::Home(_) | Body::Subs(_) => self.query.clear(),
            _ => {}
        }
        let Some(top) = self.top() else { return };
        match &top.body {
            Body::Watch(w) => {
                let (vid, resume) = (w.video.id.clone(), w.resume);
                self.start_playback(id, &vid, resume);
            }
            Body::History(_) => {
                let items = library().history();
                if let Some(Page {
                    body: Body::History(v),
                    ..
                }) = self.top_mut()
                {
                    *v = items;
                }
            }
            Body::Later(_) => {
                let items = library().watch_later();
                if let Some(Page {
                    body: Body::Later(v),
                    ..
                }) = self.top_mut()
                {
                    *v = items;
                }
            }
            _ => {}
        }
    }

    /// Start (or restart after an error) whatever the top page needs from the network. On a
    /// watch page this reloads the page data only, never the video.
    fn load_top(&mut self) -> Task<Message> {
        let Some(page) = self.top_mut() else {
            return Task::none();
        };
        let id = page.id;
        page.scroll.rearm_near_end();
        match &mut page.body {
            Body::Home(h) => {
                h.load = Load::Loading;
                h.req += 1;
                let req = h.req;
                let topic = TOPICS.get(h.topic).map(|t| t.id).unwrap_or("all");
                Task::perform(client().home(topic), move |result| Message::FeedLoaded {
                    page: id,
                    req,
                    append: false,
                    result,
                })
            }
            Body::Search(s) => {
                s.load = Load::Loading;
                Task::perform(client().search(&s.query), move |result| {
                    Message::FeedLoaded {
                        page: id,
                        req: 0,
                        append: false,
                        result,
                    }
                })
            }
            Body::Watch(w) => {
                w.load = Load::Loading;
                Task::perform(client().watch(&w.video.id), move |r| {
                    Message::WatchLoaded(id, r)
                })
            }
            Body::Channel(c) => {
                c.load = Load::Loading;
                Task::perform(client().channel(&c.id), move |r| {
                    Message::ChannelLoaded(id, r)
                })
            }
            Body::Subs(s) if !s.channels.is_empty() => {
                s.load = Load::Loading;
                Task::perform(client().subscription_feed(s.channels.clone()), move |r| {
                    Message::SubsLoaded(id, r)
                })
            }
            _ => Task::none(),
        }
    }

    fn open_video(&mut self, video: Video) -> Task<Message> {
        crate::scroller::trace_event("open_video");
        library().record_watch(&video);
        let vid = video.id.clone();
        let resume = self.prefs.resume_at(&vid);
        let watch = Watch {
            video,
            load: Load::Loading,
            info: None,
            related: Vec::new(),
            comments: Vec::new(),
            comment_chars: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: Comments::NotStarted,
            comments_req: 0,
            comments_open: HashSet::new(),
            comments_layout: RefCell::new(VarList::default()),
            desc_open: false,
            desc_spans: Vec::new(),
            resume,
            comments_y: Cell::new(f32::INFINITY),
        };
        let id = self.push(Body::Watch(Box::new(watch)));
        self.start_playback(id, &vid, resume);
        self.load_top()
    }

    fn open_video_id(&mut self, id: &str) -> Task<Message> {
        let v = Video {
            id: id.to_string(),
            thumbnail: Video::thumbnail_url(id),
            ..Video::default()
        };
        self.open_video(v)
    }

    fn open_channel(&mut self, stub: ChannelRef) -> Task<Message> {
        if stub.id.is_empty() {
            return Task::none();
        }
        let id = stub.id.clone();
        self.push(Body::Channel(Channel {
            id,
            stub,
            load: Load::Loading,
            info: None,
            feed: Feed::default(),
            desc_open: false,
        }));
        self.load_top()
    }

    fn submit_search(&mut self, query: String) -> Task<Message> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Task::none();
        }
        self.query = query.clone();
        self.blur += 1;
        self.search_open = false;
        self.push(Body::Search(Search {
            query,
            load: Load::Loading,
            feed: Feed::default(),
        }));
        self.load_top()
    }

    pub fn list(&self, which: ListRef) -> &[Video] {
        match (self.top().map(|p| &p.body), which) {
            (Some(Body::Home(h)), ListRef::Main) => &h.feed.items,
            (Some(Body::Search(s)), ListRef::Main) => &s.feed.items,
            (Some(Body::Channel(c)), ListRef::Main) => &c.feed.items,
            (Some(Body::Subs(s)), ListRef::Main) => &s.feed.items,
            (Some(Body::History(v)), ListRef::Main) | (Some(Body::Later(v)), ListRef::Main) => v,
            (Some(Body::Watch(w)), _) => &w.related,
            _ => &[],
        }
    }

    fn watch(&self) -> Option<&Watch> {
        match self.top().map(|p| &p.body) {
            Some(Body::Watch(w)) => Some(w),
            _ => None,
        }
    }

    // ---- playback -------------------------------------------------------------------------

    fn start_playback(&mut self, page: u64, video_id: &str, resume: f64) {
        // Coming back to a page: keep its last picture up until the video is back there.
        self.frame = match self.saved_frame.take() {
            Some((p, f)) if p == page => Some(f),
            _ => None,
        };
        self.seeking_to = None;
        self.up_next = None;
        self.menu = None;
        self.play = PlaybackState::default();
        // A failed start is retried on the next video: `ext::player()` creates it on demand.
        let p = match ext::player() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("nt-iced: player: {e}");
                self.player_error = Some("The video player couldn't start.".into());
                return;
            }
        };
        self.player_error = None;
        ext::reset_gate();
        if self.preload.take().is_some_and(|(id, _)| id == video_id) {
            // Already loading since the mouse went down: adopt it.
            self.playing_page = Some(page);
            self.resume_pending = (resume > 1.0).then_some(resume);
            self.want_playback_mark = true;
            self.play = p.state();
            if !(self.play.speed > 0.0) {
                self.play.speed = 1.0;
            }
            self.frame_gen = p.frame_generation();
            self.frame = p.frame();
            if self.frame.is_some() {
                self.want_playback_mark = false;
                mark_on_next_frame(marks::PLAYBACK_STARTED);
            }
            return;
        }
        match p.load(video_id) {
            Ok(()) => {
                self.playing_page = Some(page);
                self.resume_pending = (resume > 1.0).then_some(resume);
                self.want_playback_mark = true;
                self.play = p.state();
                if !(self.play.speed > 0.0) {
                    self.play.speed = 1.0;
                }
                self.frame_gen = p.frame_generation();
            }
            Err(e) => self.player_error = Some(friendly(&e)),
        }
    }

    /// Start the player on a card's video while the mouse button is still down (the click
    /// completes on release, about 100 ms later). Only when nothing is playing, so pressing a
    /// related video and dragging away never stops the one being watched.
    fn preload(&mut self, list: ListRef, i: usize, hash: u64) -> Task<Message> {
        if self.playing_page.is_some() || self.fullscreen {
            return Task::none();
        }
        let Some(v) = self.list(list).get(i).filter(|v| id_hash(&v.id) == hash) else {
            return Task::none();
        };
        if self.preload.as_ref().is_some_and(|(id, _)| *id == v.id) {
            return Task::none();
        }
        let id = v.id.clone();
        let Ok(p) = ext::player() else {
            return Task::none();
        };
        if p.load(&id).is_err() {
            return Task::none();
        }
        crate::scroller::trace_event("preload");
        self.preload_seq += 1;
        let seq = self.preload_seq;
        self.preload = Some((id, seq));
        Task::future(async move {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            Message::PreloadExpire(seq)
        })
    }

    /// The video the open card menu is for, if it is still where it was; closes the menu.
    fn menu_video(&mut self) -> Option<&Video> {
        let m = self.card_menu.take()?;
        self.list(m.list)
            .get(m.index)
            .filter(|v| id_hash(&v.id) == m.hash)
    }

    fn on_player(&mut self) -> Task<Message> {
        ext::player_taken();
        let Some(p) = ext::existing_player() else {
            return Task::none();
        };
        let was = self.play.status.clone();
        self.play = p.state();
        if !(self.play.speed > 0.0) {
            self.play.speed = 1.0;
        }
        if self.playing_page.is_none() {
            self.frame = None;
            return Task::none();
        }
        // While a resume is pending, the picture and clock stay at the resume point instead of
        // showing the start of the video for a moment.
        if let Some(t) = self.seeking_to
            && (self.play.position >= t - 1.5 || self.play.status == Status::Ended)
        {
            self.seeking_to = None;
        }
        let holding = self.resume_pending.or(self.seeking_to);
        if let Some(t) = holding {
            self.play.position = self.play.position.max(t);
        }
        let g = p.frame_generation();
        if g != self.frame_gen && holding.is_none() {
            self.frame_gen = g;
            if let Some(f) = p.frame() {
                let page_video = self.watch().map(|w| w.video.id.as_str());
                if self.play.source.as_deref() != page_video {
                    crate::scroller::trace_event("stale_frame");
                }
                self.frame = Some(f);
                if self.want_playback_mark {
                    self.want_playback_mark = false;
                    mark_on_next_frame(marks::PLAYBACK_STARTED);
                }
            }
        }
        if let Some(t) = self.resume_pending
            && self.play.status == Status::Ready
        {
            p.seek(t);
            self.resume_pending = None;
            self.seeking_to = Some(t);
        }
        if self.play.status == Status::Ended && was != Status::Ended {
            return self.video_ended();
        }
        Task::none()
    }

    fn video_ended(&mut self) -> Task<Message> {
        let Some(page) = self.playing_page else {
            return Task::none();
        };
        if let Some(w) = self.watch() {
            let id = w.video.id.clone();
            self.prefs
                .record(&id, self.play.duration, self.play.duration);
            self.prefs.save();
        }
        if self.watch().is_none_or(|w| w.related.is_empty()) {
            return Task::none();
        }
        self.up_seq += 1;
        let counting = self.prefs.autoplay;
        self.up_next = Some(UpNext {
            page,
            remaining: 5,
            counting,
            seq: self.up_seq,
        });
        if counting {
            self.tick_up_next()
        } else {
            Task::none()
        }
    }

    fn tick_up_next(&self) -> Task<Message> {
        let seq = self.up_seq;
        Task::future(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Message::UpNextTick(seq)
        })
    }

    fn player_do(&self, f: impl FnOnce(&nt_core::Player)) {
        if self.playing_page.is_some()
            && let Some(p) = ext::existing_player()
        {
            f(p);
        }
    }

    fn bezel(&mut self, icon: Icon) {
        self.bezel = Some((icon, Instant::now()));
    }

    // ---- infinite scroll ------------------------------------------------------------------

    fn maybe_more(&mut self) -> Task<Message> {
        let Some(page) = self.top_mut() else {
            return Task::none();
        };
        let id = page.id;
        let near = page.scroll.near_end();
        let (offset, vh) = (page.scroll.offset(), page.scroll.viewport().height);
        let feed = match &mut page.body {
            Body::Home(h) if h.load == Load::Ready => Some((&mut h.feed, h.req)),
            Body::Search(s) if s.load == Load::Ready => Some((&mut s.feed, 0)),
            Body::Channel(c) if c.load == Load::Ready => Some((&mut c.feed, 0)),
            Body::Watch(w) => {
                let near_comments = w.comments_y.get() <= offset + 2.5 * vh;
                let go = match w.comments_state {
                    Comments::NotStarted => near_comments && w.load == Load::Ready,
                    Comments::Idle => near && w.comments_next.is_some(),
                    _ => false,
                };
                return if go {
                    Self::load_comments(id, w)
                } else {
                    Task::none()
                };
            }
            _ => None,
        };
        let Some((feed, req)) = feed else {
            return Task::none();
        };
        if !near || feed.more != Some(More::Idle) {
            return Task::none();
        }
        let Some(next) = feed.next.clone() else {
            return Task::none();
        };
        feed.more = Some(More::Loading);
        Task::perform(client().more_videos(&next), move |result| {
            Message::FeedLoaded {
                page: id,
                req,
                append: true,
                result,
            }
        })
    }

    fn load_comments(id: u64, w: &mut Watch) -> Task<Message> {
        if w.comments_state == Comments::Loading {
            return Task::none();
        }
        let next = if w.comments.is_empty() {
            w.info.as_ref().and_then(|i| i.comments.clone())
        } else {
            w.comments_next.clone()
        };
        let Some(next) = next else {
            return Task::none();
        };
        w.comments_state = Comments::Loading;
        w.comments_req += 1;
        let req = w.comments_req;
        Task::perform(client().more_comments(&next), move |r| {
            Message::CommentsLoaded(id, req, r)
        })
    }

    fn close_suggestions(&mut self) {
        self.sugg_open = false;
        self.sugg_sel = None;
        self.sugg_seq += 1;
    }

    fn toast(&mut self, text: String) -> Task<Message> {
        self.toast_seq += 1;
        let seq = self.toast_seq;
        self.toast = Some((text, seq));
        Task::future(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Message::ToastDone(seq)
        })
    }

    fn set_fullscreen(&mut self, on: bool) -> Task<Message> {
        if on && self.watch().is_none() {
            return Task::none();
        }
        self.fullscreen = on;
        let mode = if on {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        match self.window {
            Some(id) => window::set_mode(id, mode),
            None => Task::none(),
        }
    }

    fn quit(&mut self) -> Task<Message> {
        self.leave_top();
        if let Some(p) = ext::existing_player() {
            p.stop();
        }
        iced::exit()
    }

    // ---- update ---------------------------------------------------------------------------

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Nav(root) => {
                let t = self.set_fullscreen(false);
                Task::batch([t, self.replace_root(root)])
            }
            Message::Back => {
                let t = self.set_fullscreen(false);
                Task::batch([t, self.back()])
            }
            Message::Forward => self.go_forward(),
            Message::Quit => self.quit(),
            Message::Pilot(cmd) => self.pilot(cmd),
            Message::Ext(Ext::Image(url, w, r)) => match self.images.loaded(url, w, r) {
                Some(h) => iced_runtime::image::allocate(h.clone())
                    .map(move |r| Message::Allocated(h.clone(), r)),
                None => Task::none(),
            },
            Message::Allocated(h, r) => {
                match r {
                    Ok(a) => self.images.allocated(&h, a),
                    Err(e) => eprintln!("nt-iced: image upload: {e}"),
                }
                Task::none()
            }
            Message::Ext(Ext::Player) => self.on_player(),
            Message::ScrollMoved => self.maybe_more(),
            Message::Topic(i) => {
                if i >= TOPICS.len() {
                    return Task::none();
                }
                if let Some(Page {
                    body: Body::Home(h),
                    scroll,
                    ..
                }) = self.top_mut()
                {
                    if h.topic == i && h.load == Load::Ready {
                        return Task::none();
                    }
                    h.topic = i;
                    h.feed = Feed::default();
                    scroll.with(|a, _| a.jump_to(0.0));
                }
                self.load_top()
            }
            Message::Open(list, i, hash) => match self.list(list).get(i) {
                Some(v) if id_hash(&v.id) == hash => {
                    let v = v.clone();
                    self.card_menu = None;
                    self.open_video(v)
                }
                _ => Task::none(),
            },
            Message::Preload(list, i, hash) => self.preload(list, i, hash),
            Message::PreloadExpire(seq) => {
                if self.preload.as_ref().is_some_and(|(_, s)| *s == seq) {
                    self.preload = None;
                    if self.playing_page.is_none()
                        && let Some(p) = ext::existing_player()
                    {
                        p.stop();
                    }
                }
                Task::none()
            }
            Message::OpenChannelOf(list, i) => {
                let Some(v) = self.list(list).get(i) else {
                    return Task::none();
                };
                let Some(id) = v.channel_id.clone() else {
                    return Task::none();
                };
                let stub = ChannelRef {
                    id,
                    name: v.channel.clone(),
                    avatar: v.channel_avatar.clone(),
                    subscribers: None,
                };
                self.open_channel(stub)
            }
            Message::OpenChannel(c) => self.open_channel(c),
            Message::OpenWatchChannel => {
                let stub = self.watch().and_then(|w| match &w.info {
                    Some(i) if !i.channel.id.is_empty() => Some(i.channel.clone()),
                    _ => w.video.channel_id.clone().map(|id| ChannelRef {
                        id,
                        name: w.video.channel.clone(),
                        avatar: w.video.channel_avatar.clone(),
                        subscribers: None,
                    }),
                });
                let t = self.set_fullscreen(false);
                let open = stub
                    .map(|s| self.open_channel(s))
                    .unwrap_or_else(Task::none);
                Task::batch([t, open])
            }
            Message::FeedLoaded {
                page,
                req,
                append,
                result,
            } => {
                let prefs = &self.prefs;
                let result = result.map(|mut pg| {
                    pg.items.retain(|v| !prefs.is_hidden(&v.id));
                    pg
                });
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let (load, feed, mark) = match &mut p.body {
                    Body::Home(h) if h.req == req => (&mut h.load, &mut h.feed, marks::HOME_LOADED),
                    Body::Search(s) => (&mut s.load, &mut s.feed, marks::SEARCH_LOADED),
                    Body::Channel(c) => (&mut c.load, &mut c.feed, marks::CHANNEL_LOADED),
                    _ => return Task::none(),
                };
                match (result, append) {
                    (Ok(pg), false) => {
                        feed.set(pg);
                        *load = Load::Ready;
                        mark_on_next_frame(mark);
                        ext::prewarm_player(feed.items.first().map(|v| v.id.clone()));
                    }
                    (Ok(pg), true) => {
                        // A "more" page for a feed that was reset meanwhile is stale.
                        if feed.more != Some(More::Loading) {
                            return Task::none();
                        }
                        feed.append(pg);
                        mark_on_next_frame(marks::MORE_LOADED);
                    }
                    (Err(e), false) => *load = Load::Failed(friendly(&e)),
                    (Err(e), true) => {
                        if feed.more == Some(More::Loading) {
                            feed.more = Some(More::Failed(friendly(&e)));
                        }
                    }
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::WatchLoaded(page, r) => {
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let Body::Watch(w) = &mut p.body else {
                    return Task::none();
                };
                match r {
                    Ok(mut info) => {
                        // Some watch responses carry no channel; the card that opened the
                        // video knows it (NOTES.md, Core requests).
                        let ch = &mut info.channel;
                        if ch.id.is_empty() {
                            ch.id = w.video.channel_id.clone().unwrap_or_default();
                        }
                        if ch.name.is_empty() {
                            ch.name = w.video.channel.clone();
                        }
                        if ch.avatar.is_none() {
                            ch.avatar = w.video.channel_avatar.clone();
                        }
                        if w.video.title.is_empty() {
                            w.video.title = info.title.clone();
                            w.video.channel = info.channel.name.clone();
                        }
                        w.related = std::mem::take(&mut info.related.items);
                        w.desc_spans = crate::player_view::linkify(&info.description);
                        w.info = Some(info);
                        w.load = Load::Ready;
                        mark_on_next_frame(marks::WATCH_LOADED);
                    }
                    Err(e) => w.load = Load::Failed(friendly(&e)),
                }
                p.scroll.rearm_near_end();
                // The comments may already be within reach; start them now if so.
                if self.top_id() == page {
                    return self.maybe_more();
                }
                Task::none()
            }
            Message::CommentsLoaded(page, req, r) => {
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let Body::Watch(w) = &mut p.body else {
                    return Task::none();
                };
                if req != w.comments_req || w.comments_state != Comments::Loading {
                    return Task::none();
                }
                match r {
                    Ok(cp) => {
                        if cp.total.is_some() {
                            w.comments_total = cp.total;
                        }
                        w.comment_chars.extend(
                            cp.items
                                .iter()
                                .map(|c| c.text.chars().count().min(u32::MAX as usize) as u32),
                        );
                        w.comments.extend(cp.items);
                        w.comments_next = cp.next;
                        w.comments_state = Comments::Idle;
                        mark_on_next_frame(marks::COMMENTS_LOADED);
                    }
                    Err(e) => w.comments_state = Comments::Failed(friendly(&e)),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::ChannelLoaded(page, r) => {
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let Body::Channel(c) = &mut p.body else {
                    return Task::none();
                };
                match r {
                    Ok(mut info) => {
                        c.feed.set(std::mem::take(&mut info.videos));
                        c.info = Some(info);
                        c.load = Load::Ready;
                        mark_on_next_frame(marks::CHANNEL_LOADED);
                    }
                    Err(e) => c.load = Load::Failed(friendly(&e)),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::SubsLoaded(page, r) => {
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let Body::Subs(s) = &mut p.body else {
                    return Task::none();
                };
                match r {
                    Ok(items) => {
                        s.feed.set(nt_core::Page { items, next: None });
                        s.load = Load::Ready;
                    }
                    Err(e) => s.load = Load::Failed(friendly(&e)),
                }
                Task::none()
            }
            Message::Retry => self.load_top(),
            Message::RetryMore => {
                let Some(page) = self.top_mut() else {
                    return Task::none();
                };
                page.scroll.rearm_near_end();
                match &mut page.body {
                    Body::Home(Home { feed, .. })
                    | Body::Search(Search { feed, .. })
                    | Body::Channel(Channel { feed, .. }) => {
                        feed.more = Some(More::Idle);
                    }
                    _ => {}
                }
                self.maybe_more()
            }
            Message::RetryComments => {
                let id = self.top_id();
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) => {
                        w.comments_state = if w.comments.is_empty() {
                            Comments::NotStarted
                        } else {
                            Comments::Idle
                        };
                        Self::load_comments(id, w)
                    }
                    _ => Task::none(),
                }
            }
            Message::LoadComments => {
                let id = self.top_id();
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) => Self::load_comments(id, w),
                    _ => Task::none(),
                }
            }
            Message::SearchInput(q) => {
                self.query = q;
                self.sugg_seq += 1;
                self.sugg_sel = None;
                let seq = self.sugg_seq;
                if self.query.trim().is_empty() {
                    self.sugg_open = false;
                    self.suggestions.clear();
                    return Task::none();
                }
                Task::future(async move {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    Message::SuggestFire(seq)
                })
            }
            Message::SuggestFire(seq) if seq == self.sugg_seq => {
                let asked = self.query.clone();
                Task::perform(client().suggestions(&asked), move |r| {
                    Message::Suggestions(seq, r)
                })
            }
            Message::SuggestFire(_) => Task::none(),
            Message::Suggestions(seq, r) => {
                if seq != self.sugg_seq {
                    return Task::none();
                }
                match r {
                    Ok(mut list) => {
                        list.truncate(10);
                        self.sugg_open = !list.is_empty() && self.search_focused;
                        self.suggestions = list;
                        self.sugg_for = self.query.trim().to_lowercase();
                        self.sugg_sel = None;
                    }
                    Err(e) => eprintln!("nt-iced: suggestions: {e}"),
                }
                Task::none()
            }
            Message::SuggestNav(d) => {
                if !self.sugg_open || self.suggestions.is_empty() {
                    return Task::none();
                }
                let n = self.suggestions.len() as i32;
                let cur = self
                    .sugg_sel
                    .map(|i| i as i32)
                    .unwrap_or(if d > 0 { -1 } else { n });
                let next = cur + d;
                self.sugg_sel = (0..n).contains(&next).then_some(next as usize);
                Task::none()
            }
            Message::SuggestClose => {
                self.close_suggestions();
                Task::none()
            }
            Message::SuggestPick(i) => match self.suggestions.get(i).cloned() {
                Some(q) => self.submit_search(q),
                None => Task::none(),
            },
            Message::SearchSubmit => {
                let q = match self.sugg_sel.and_then(|i| self.suggestions.get(i)) {
                    Some(s) if self.sugg_open => s.clone(),
                    _ => self.query.clone(),
                };
                self.submit_search(q)
            }
            Message::SearchFocus(f) => {
                self.search_focused = f;
                if !f {
                    self.sugg_open = false;
                }
                Task::none()
            }
            Message::SearchOpen(open) => {
                self.search_open = open;
                if open {
                    iced::widget::operation::focus(SEARCH_ID)
                } else {
                    self.blur += 1;
                    self.close_suggestions();
                    Task::none()
                }
            }
            Message::FocusSearch => {
                if self.fullscreen {
                    return Task::none();
                }
                if self.win.width < 656.0 {
                    self.search_open = true;
                }
                iced::widget::operation::focus(SEARCH_ID)
            }
            Message::ToggleSubscribe => {
                let ch = match self.top().map(|p| &p.body) {
                    Some(Body::Watch(w)) => w.info.as_ref().map(|i| i.channel.clone()),
                    Some(Body::Channel(c)) => {
                        c.info.as_ref().map(|i| i.as_ref()).or(Some(c.stub.clone()))
                    }
                    _ => None,
                };
                if let Some(ch) = ch.filter(|c| !c.id.is_empty()) {
                    let now = library().toggle_subscription(&ch);
                    let text = if now {
                        format!("Subscribed to {}", ch.name)
                    } else {
                        "Subscription removed".to_string()
                    };
                    return self.toast(text);
                }
                Task::none()
            }
            Message::ToggleLater => {
                let Some(w) = self.watch() else {
                    return Task::none();
                };
                let mut v = w
                    .info
                    .as_ref()
                    .map(|i| i.as_video())
                    .unwrap_or_else(|| w.video.clone());
                if v.duration.is_none() {
                    v.duration = w.video.duration.clone();
                }
                v.is_live = w.video.is_live;
                let now = library().toggle_watch_later(&v);
                self.toast(if now {
                    "Saved to Watch Later".into()
                } else {
                    "Removed from Watch Later".into()
                })
            }
            Message::Share => {
                let Some(w) = self.watch() else {
                    return Task::none();
                };
                let url = watch_url(&w.video.id);
                Task::batch([
                    iced::clipboard::write(url),
                    self.toast("Link copied to clipboard".into()),
                ])
            }
            Message::ClearHistory => {
                library().clear_history();
                if let Some(Page {
                    body: Body::History(v),
                    ..
                }) = self.top_mut()
                {
                    v.clear();
                }
                Task::none()
            }
            Message::ToggleDesc => {
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) => w.desc_open = !w.desc_open,
                    Some(Body::Channel(c)) => c.desc_open = !c.desc_open,
                    _ => {}
                }
                Task::none()
            }
            Message::ToggleComment(i) => {
                if let Some(Page {
                    body: Body::Watch(w),
                    ..
                }) = self.top_mut()
                    && !w.comments_open.remove(&i)
                {
                    w.comments_open.insert(i);
                }
                Task::none()
            }
            Message::DescLink(link) => match link {
                Link::Seek(t) => {
                    self.player_do(|p| p.seek(t));
                    if let Some(Page { scroll, .. }) = self.top_mut() {
                        scroll.with(|a, now| a.scroll_to(0.0, now));
                    }
                    Task::none()
                }
                Link::Search(q) => self.submit_search(q),
                Link::Url(url) => match youtube_video_id(&url) {
                    Some(id) => self.open_video_id(&id),
                    None => Task::batch([
                        iced::clipboard::write(url),
                        self.toast("Link copied to clipboard".into()),
                    ]),
                },
            },
            Message::CardMenu(list, index) | Message::CardContext(list, index) => {
                let context = matches!(message, Message::CardContext(..));
                let Some(hash) = self.list(list).get(index).map(|v| id_hash(&v.id)) else {
                    return Task::none();
                };
                self.card_menu = Some(CardMenu {
                    list,
                    index,
                    hash,
                    at: last_cursor(),
                    context,
                });
                Task::none()
            }
            Message::CardMenuClose => {
                self.card_menu = None;
                Task::none()
            }
            Message::CardSaveLater => {
                let Some(v) = self.menu_video().cloned() else {
                    return Task::none();
                };
                let now = library().toggle_watch_later(&v);
                self.toast(if now {
                    "Saved to Watch Later".into()
                } else {
                    "Removed from Watch Later".into()
                })
            }
            Message::CardCopyLink => {
                let Some(v) = self.menu_video() else {
                    return Task::none();
                };
                let url = watch_url(&v.id);
                Task::batch([
                    iced::clipboard::write(url),
                    self.toast("Link copied to clipboard".into()),
                ])
            }
            Message::CardOpenChannel => {
                let Some(m) = self.card_menu.clone() else {
                    return Task::none();
                };
                if self.menu_video().is_none() {
                    return Task::none();
                }
                self.update(Message::OpenChannelOf(m.list, m.index))
            }
            Message::CardNotInterested => {
                let Some(m) = self.card_menu.clone() else {
                    return Task::none();
                };
                let Some(id) = self.menu_video().map(|v| v.id.clone()) else {
                    return Task::none();
                };
                self.prefs.hide(&id);
                self.prefs.save();
                if let Some(p) = self.top_mut() {
                    let list = match (&mut p.body, m.list) {
                        (Body::Home(h), ListRef::Main) => Some(&mut h.feed.items),
                        (Body::Search(s), ListRef::Main) => Some(&mut s.feed.items),
                        (Body::Channel(c), ListRef::Main) => Some(&mut c.feed.items),
                        (Body::Subs(s), ListRef::Main) => Some(&mut s.feed.items),
                        (Body::Watch(w), ListRef::Related) => Some(&mut w.related),
                        _ => None,
                    };
                    if let Some(items) = list {
                        items.retain(|v| v.id != id);
                    }
                }
                self.toast("Video removed. You won't see it again.".into())
            }
            Message::Toast(t) => self.toast(t),
            Message::ToastDone(seq) => {
                if self.toast.as_ref().is_some_and(|(_, s)| *s == seq) {
                    self.toast = None;
                }
                Task::none()
            }
            Message::PlayPause | Message::ClickVideo => {
                if self.menu.is_some() && matches!(message, Message::ClickVideo) {
                    self.menu = None;
                    return Task::none();
                }
                let paused = self.play.paused || self.play.status == Status::Ended;
                self.player_do(|p| p.toggle_pause());
                self.bezel(if paused { Icon::PlayArrow } else { Icon::Pause });
                if paused {
                    self.up_next = None;
                }
                Task::none()
            }
            Message::Replay => {
                self.up_next = None;
                self.player_do(|p| {
                    p.seek(0.0);
                    p.play();
                });
                Task::none()
            }
            Message::SeekTo(t) => {
                let t = t.max(0.0);
                self.player_do(|p| p.seek(t));
                self.play.position = t;
                self.up_next = None;
                Task::none()
            }
            Message::SeekFrac(f) => {
                let d = self.play.duration;
                if d > 0.0 && !self.play.is_live {
                    let t = (f.clamp(0.0, 1.0) as f64) * d;
                    return self.update(Message::SeekTo(t));
                }
                Task::none()
            }
            Message::SeekBy(s) => {
                self.player_do(|p| p.seek_by(s));
                self.bezel(if s < 0.0 {
                    Icon::ChevronLeft
                } else {
                    Icon::ChevronRight
                });
                Task::none()
            }
            Message::SeekPercent(n) => self.update(Message::SeekFrac(f32::from(n.min(9)) / 10.0)),
            Message::FrameStep(d) => {
                if self.play.paused {
                    // The core has no frame-step command; one frame at 30 fps is the closest.
                    self.player_do(|p| p.seek_by(f64::from(d) / 30.0));
                }
                Task::none()
            }
            Message::SpeedStep(d) => {
                if self.play.is_live {
                    return Task::none();
                }
                let cur = SPEEDS
                    .iter()
                    .position(|s| (*s - self.play.speed).abs() < 0.01)
                    .unwrap_or(3) as i32;
                let next = (cur + d).clamp(0, SPEEDS.len() as i32 - 1) as usize;
                let speed = SPEEDS.get(next).copied().unwrap_or(1.0);
                self.update(Message::SetSpeed(speed))
            }
            Message::SetVolume(v) => {
                self.player_do(|p| {
                    p.set_volume(v as f64);
                    p.set_muted(false);
                });
                self.play.volume = v as f64;
                Task::none()
            }
            Message::VolumeBy(d) => {
                let v = (self.play.volume + d).clamp(0.0, 100.0);
                self.player_do(|p| p.set_volume(v));
                self.bezel(if d < 0.0 {
                    Icon::VolumeDown
                } else {
                    Icon::VolumeUp
                });
                Task::none()
            }
            Message::ToggleMute => {
                let muted = self.play.muted;
                self.player_do(|p| p.set_muted(!muted));
                self.bezel(if muted {
                    Icon::VolumeUp
                } else {
                    Icon::VolumeOff
                });
                Task::none()
            }
            Message::VolumeHover(h) => {
                self.volume_hover = h;
                Task::none()
            }
            Message::Menu(m) => {
                self.menu = m;
                Task::none()
            }
            Message::SetSpeed(s) => {
                self.player_do(|p| p.set_speed(s));
                self.play.speed = s;
                if self.menu == Some(MenuPage::Speed) {
                    self.menu = Some(MenuPage::Main);
                }
                Task::none()
            }
            Message::SetQuality(q) => {
                self.player_do(|p| p.set_quality(q));
                if self.menu == Some(MenuPage::Quality) {
                    self.menu = Some(MenuPage::Main);
                }
                Task::none()
            }
            Message::ToggleAutoplay => {
                self.prefs.autoplay = !self.prefs.autoplay;
                self.prefs.save();
                if let Some(u) = &mut self.up_next
                    && !self.prefs.autoplay
                {
                    u.counting = false;
                }
                Task::none()
            }
            Message::ToggleFullscreen => {
                let on = !self.fullscreen;
                self.set_fullscreen(on)
            }
            Message::ModeIs(mode) => {
                self.fullscreen = mode == window::Mode::Fullscreen;
                Task::none()
            }
            Message::ToggleTheatre => {
                self.prefs.theatre = !self.prefs.theatre;
                self.prefs.save();
                Task::none()
            }
            Message::Escape => {
                if self.menu.is_some() {
                    self.menu = None;
                    Task::none()
                } else if self.fullscreen {
                    self.set_fullscreen(false)
                } else {
                    self.card_menu = None;
                    Task::none()
                }
            }
            Message::RetryPlayback => {
                let Some(top) = self.top() else {
                    return Task::none();
                };
                if let Body::Watch(w) = &top.body {
                    let (id, vid, at) =
                        (top.id, w.video.id.clone(), self.play.position.max(w.resume));
                    self.start_playback(id, &vid, at);
                }
                Task::none()
            }
            Message::UpNextTick(seq) => {
                let Some(u) = &mut self.up_next else {
                    return Task::none();
                };
                if u.seq != seq || !u.counting {
                    return Task::none();
                }
                u.remaining = u.remaining.saturating_sub(1);
                if u.remaining == 0 {
                    return self.update(Message::UpNextPlay);
                }
                self.tick_up_next()
            }
            Message::UpNextCancel => {
                if let Some(u) = &mut self.up_next {
                    u.counting = false;
                }
                self.up_next = None;
                Task::none()
            }
            Message::UpNextPlay => {
                let next = self
                    .up_next
                    .take()
                    .filter(|u| Some(u.page) == self.playing_page)
                    .and_then(|_| self.watch())
                    .and_then(|w| w.related.first().cloned());
                match next {
                    Some(v) => self.open_video(v),
                    None => Task::none(),
                }
            }
            Message::WindowSize(s) => {
                self.win = Size::new(s.width.max(1.0), s.height.max(1.0));
                match self.window {
                    Some(id) => window::mode(id).map(Message::ModeIs),
                    None => Task::none(),
                }
            }
            Message::WindowId(id) => {
                if id.is_some() {
                    self.window = id;
                }
                Task::none()
            }
            Message::Scale(s) => {
                if s.is_finite() && s > 0.0 {
                    self.scale = s;
                }
                Task::none()
            }
        }
    }

    /// Keys the watch page takes before scrolling sees them.
    pub fn watch_key_message(key: &keyboard::Key, mods: keyboard::Modifiers) -> Option<Message> {
        if mods.control() || mods.alt() || mods.logo() {
            return None;
        }
        Some(match key.as_ref() {
            keyboard::Key::Named(Named::Space) | keyboard::Key::Character("k") => {
                Message::PlayPause
            }
            keyboard::Key::Character("j") => Message::SeekBy(-10.0),
            keyboard::Key::Character("l") => Message::SeekBy(10.0),
            keyboard::Key::Named(Named::ArrowLeft) => Message::SeekBy(-5.0),
            keyboard::Key::Named(Named::ArrowRight) => Message::SeekBy(5.0),
            keyboard::Key::Named(Named::ArrowUp) => Message::VolumeBy(5.0),
            keyboard::Key::Named(Named::ArrowDown) => Message::VolumeBy(-5.0),
            keyboard::Key::Named(Named::Home) => Message::SeekTo(0.0),
            keyboard::Key::Named(Named::End) => Message::SeekFrac(0.999),
            keyboard::Key::Named(Named::Escape) => Message::Escape,
            keyboard::Key::Character("m") => Message::ToggleMute,
            keyboard::Key::Character("f") => Message::ToggleFullscreen,
            keyboard::Key::Character("t") => Message::ToggleTheatre,
            keyboard::Key::Character("<") => Message::SpeedStep(-1),
            keyboard::Key::Character(">") => Message::SpeedStep(1),
            keyboard::Key::Character(",") if mods.shift() => Message::SpeedStep(-1),
            keyboard::Key::Character(".") if mods.shift() => Message::SpeedStep(1),
            keyboard::Key::Character(",") => Message::FrameStep(-1),
            keyboard::Key::Character(".") => Message::FrameStep(1),
            keyboard::Key::Character(c) => {
                let d = c.chars().next().filter(|c| c.is_ascii_digit())?;
                Message::SeekPercent(d.to_digit(10).unwrap_or(0) as u8)
            }
            _ => return None,
        })
    }

    fn pilot(&mut self, cmd: Command) -> Task<Message> {
        let scroll = |app: &App, f: &dyn Fn(&mut nt_core::scroll::ScrollAnimator, f64)| {
            if let Some(p) = app.top() {
                p.scroll.with(|a, now| f(a, now));
            }
        };
        match cmd {
            Command::Home { topic } => {
                let idx = topic
                    .and_then(|t| TOPICS.iter().position(|x| x.id == t))
                    .unwrap_or(0);
                self.leave_top();
                let page = self.new_page(Body::Home(Home {
                    topic: idx,
                    load: Load::Loading,
                    feed: Feed::default(),
                    req: 0,
                    chip_offset: Cell::new(0.0),
                }));
                self.pages = vec![page];
                self.forward.clear();
                self.load_top()
            }
            Command::Search { query } => {
                self.query = query;
                self.update(Message::SearchSubmit)
            }
            Command::OpenVideo { index } => {
                let list = if self.watch().is_some() {
                    ListRef::Related
                } else {
                    ListRef::Main
                };
                match self.list(list).get(index) {
                    Some(v) => {
                        let h = id_hash(&v.id);
                        self.update(Message::Open(list, index, h))
                    }
                    None => Task::none(),
                }
            }
            Command::OpenChannel => self.update(Message::OpenWatchChannel),
            Command::Back => self.update(Message::Back),
            Command::Scroll { ticks } => {
                scroll(self, &|a, now| a.wheel_ticks(ticks, now));
                Task::none()
            }
            Command::ScrollPixels { px } => {
                scroll(self, &|a, now| a.wheel_pixels(px, now));
                Task::none()
            }
            Command::Fling { velocity } => {
                scroll(self, &|a, now| a.fling(velocity, now));
                Task::none()
            }
            Command::ScrollToTop => {
                scroll(self, &|a, _| a.jump_to(0.0));
                Task::none()
            }
            Command::TogglePause => self.update(Message::PlayPause),
            Command::LoadMoreComments => self.update(Message::LoadComments),
            Command::Quit => self.quit(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(ext::events),
            Subscription::run(ext::autopilot),
            event::listen_with(|e, status, id| match e {
                iced::Event::Window(window::Event::Resized(s)) => Some(Message::WindowSize(s)),
                iced::Event::Window(window::Event::Opened { size, .. }) => {
                    let _ = id;
                    Some(Message::WindowSize(size))
                }
                iced::Event::Window(window::Event::CloseRequested) => Some(Message::Quit),
                iced::Event::Window(window::Event::Rescaled(s)) => Some(Message::Scale(s)),
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)) => {
                    Some(Message::Back)
                }
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Forward)) => {
                    Some(Message::Forward)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    global_key(&key, modifiers, status)
                }
                _ => None,
            }),
        ])
    }

    pub fn is_subscribed(&self, id: &str) -> bool {
        !id.is_empty() && library().is_subscribed(id)
    }
}

/// Keys that work on every page, whatever has focus.
fn global_key(
    key: &keyboard::Key,
    mods: keyboard::Modifiers,
    status: event::Status,
) -> Option<Message> {
    match key.as_ref() {
        keyboard::Key::Character("q") if mods.control() || mods.logo() => Some(Message::Quit),
        keyboard::Key::Character("w") if mods.logo() => Some(Message::Quit),
        keyboard::Key::Named(Named::ArrowLeft) if mods.alt() => Some(Message::Back),
        keyboard::Key::Named(Named::ArrowRight) if mods.alt() => Some(Message::Forward),
        keyboard::Key::Character("/")
            if status == event::Status::Ignored
                && !(mods.control() || mods.alt() || mods.logo()) =>
        {
            Some(Message::FocusSearch)
        }
        keyboard::Key::Named(Named::Escape) if status == event::Status::Ignored => {
            Some(Message::Escape)
        }
        _ => None,
    }
}

/// The video id of a YouTube watch link, if it is one.
fn youtube_video_id(url: &str) -> Option<String> {
    let rest = url
        .split_once("youtube.com/watch?")
        .map(|(_, r)| r)
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("v=")))
        .or_else(|| url.split_once("youtu.be/").map(|(_, r)| r))?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (id.len() == 11).then_some(id)
}
