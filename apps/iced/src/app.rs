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
    ChannelInfo, ChannelRef, Comment, CommentPage, Continuation, PlaybackState, SearchFilter,
    SearchNotice, SearchPage, SearchResult, Status, TOPICS, Video, VideoFrame, WatchInfo, client,
    library,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The search field's widget id (for the `/` shortcut).
pub const SEARCH_ID: &str = "search";
/// Pages kept behind the current one; the oldest (after the root) go first.
const MAX_PAGES: usize = 24;
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
    /// Search matches found in video transcripts (search.pronsh.dev).
    Moments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewTarget {
    pub page: u64,
    pub list: ListRef,
    pub index: usize,
    pub hash: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuPage {
    Main,
    Speed,
    Quality,
    Sleep,
    Captions,
    Translate,
    Audio,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SleepTimer {
    #[default]
    Off,
    Minutes(u32),
    EndOfVideo,
}

impl SleepTimer {
    pub const OPTIONS: [Self; 8] = [
        Self::Off,
        Self::Minutes(10),
        Self::Minutes(15),
        Self::Minutes(20),
        Self::Minutes(30),
        Self::Minutes(45),
        Self::Minutes(60),
        Self::EndOfVideo,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Off => "Off".into(),
            Self::Minutes(n) => format!("{n} min"),
            Self::EndOfVideo => "End of video".into(),
        }
    }
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
    PreviewEnter(PreviewTarget),
    PreviewLeave(PreviewTarget),
    PreviewReady(PreviewTarget, Result<(), String>),
    StopPreview,
    OpenChannelOf(ListRef, usize),
    OpenChannel(ChannelRef),
    OpenWatchChannel,
    OpenPlaylist(String),
    PlaylistLoaded(u64, u64, nt_core::Result<nt_core::PlaylistPanel>),
    PlaylistPlay(usize),
    PlaylistMore,
    PlaylistMoreLoaded(u64, nt_core::Result<nt_core::Page<Video>>),
    PreviousVideo,
    Queue(bool),
    QueueAdd,
    QueueRemove(String),
    QueuePlay(String),
    CardHideChannel,
    FeedLoaded {
        page: u64,
        req: u64,
        append: bool,
        result: nt_core::Result<nt_core::Page<Video>>,
    },
    WatchLoaded(u64, Box<nt_core::Result<WatchInfo>>),
    SearchLoaded(u64, u64, bool, Box<nt_core::Result<SearchPage>>),
    SearchFilters(bool),
    ApplySearchFilter(SearchFilter),
    CommentsLoaded(u64, u64, Option<usize>, nt_core::Result<CommentPage>),
    SortComments(usize),
    ReplyToggle(usize),
    ReplyMore(usize),
    RepliesLoaded(u64, u64, usize, nt_core::Result<CommentPage>),
    RelatedChip(usize),
    RelatedMore,
    RelatedLoaded(
        u64,
        u64,
        Option<usize>,
        nt_core::Result<nt_core::Page<Video>>,
    ),
    ChannelLoaded(u64, u64, Box<nt_core::Result<ChannelInfo>>),
    ChannelTab(nt_core::ChannelTab),
    ChannelSort(usize),
    ChannelSorted(u64, u64, usize, nt_core::Result<nt_core::Page<Video>>),
    ChannelAbout(bool),
    ChannelAboutLoaded(u64, nt_core::Result<nt_core::ChannelAbout>),
    ChannelPlaylistsLoaded(
        u64,
        u64,
        nt_core::Result<nt_core::Page<nt_core::PlaylistRef>>,
    ),
    HomeLoaded(u64, u64, nt_core::Result<nt_core::HomeFeed>),
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
    ContextClose,
    CardPlay,
    PlayerContext,
    CopyVideoUrl(bool),
    Stats(bool),
    Guide,
    Shortcuts(bool),
    Tab(bool),
    Sleep(SleepTimer),
    SleepFired(u64),
    CardNotInterested,
    CardOpenChannel,
    CardSaveLater,
    CardCopyLink,
    Toast(String),
    ToastDone(u64),
    // Player.
    ToggleCaptions,
    SetCaptions(Option<(usize, Option<String>)>),
    PlayerSettingDone(u64, nt_core::Result<()>),
    SetAudio(String),
    Normalize,
    Loop,
    ScrubHover(Option<f64>),
    ScrubImage(u64, nt_core::Result<Arc<nt_core::RgbaImage>>),
    ShowChapters,
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
    ResumeLast,
    UpNextTick(u64),
    UpNextCancel,
    UpNextPlay,
    Replay,
    WindowSize(Size),
    WindowId(Option<window::Id>),
    Scale(f32),
    /// The player's controls appeared (true) or hid (false): captions move with them.
    ControlsShown(bool),
    CaptionFrame(Instant),
    /// Test-only (`NT_TEST_CYCLE`): one step of a scripted UI toggle loop.
    TestCycle,
    SurfaceAttached,
    /// Open a watch page after the GPU surface was attached: (video, start, playlist).
    OpenWatch(Box<(Video, f64, Option<Arc<nt_core::PlaylistPanel>>)>),
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
    pub chips: Vec<nt_core::Chip>,
    show_initial: bool,
    pub load: Load,
    pub feed: Feed,
    pub req: u64,
    pub chip_offset: Cell<f32>,
}

pub struct Search {
    pub query: String,
    pub load: Load,
    pub feed: Feed,
    pub notices: Vec<SearchNotice>,
    /// The transcript fallback, shown when YouTube itself has no video matches.
    pub moments: Feed,
    pub moments_total: Option<u64>,
    pub chips: Vec<nt_core::SearchOption>,
    pub filters: Vec<nt_core::SearchFilterGroup>,
    pub filter: SearchFilter,
    pub entries: Vec<SearchEntry>,
    req: u64,
}

pub enum SearchEntry {
    Video(usize),
    Channel(nt_core::SearchChannel),
    Playlist(nt_core::PlaylistRef),
    Shelf(String),
}

impl Search {
    fn add_entries(&mut self, entries: Vec<SearchResult>, indexes: &HashMap<String, usize>) {
        for entry in entries {
            match entry {
                SearchResult::Video(video) => {
                    if let Some(index) = indexes.get(&video.id) {
                        self.entries.push(SearchEntry::Video(*index));
                    }
                }
                SearchResult::Channel(channel) => self.entries.push(SearchEntry::Channel(channel)),
                SearchResult::Playlist(playlist) => {
                    self.entries.push(SearchEntry::Playlist(playlist))
                }
                SearchResult::Shelf(shelf) => {
                    if !shelf.title.is_empty() {
                        self.entries.push(SearchEntry::Shelf(shelf.title));
                    }
                    self.add_entries(shelf.items, indexes);
                }
            }
        }
    }
}

/// `FeedLoaded::req` on a search page: which section a "more" page belongs to.
const SEARCH_RESULTS: u64 = 0;
const SEARCH_MOMENTS: u64 = 1;

pub struct Channel {
    pub id: String,
    pub stub: ChannelRef,
    pub load: Load,
    pub info: Option<ChannelInfo>,
    pub feed: Feed,
    pub desc_open: bool,
    pub tab: nt_core::ChannelTab,
    pub sort: Option<usize>,
    sort_pending: Option<usize>,
    req: u64,
    pub about: Option<nt_core::ChannelAbout>,
    pub about_load: Load,
    pub playlists_more: More,
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

#[derive(Default)]
pub struct Replies {
    pub open: bool,
    pub items: Vec<Comment>,
    next: Option<Continuation>,
    pub more: Option<More>,
}

impl Replies {
    pub fn has_more(&self) -> bool {
        self.next.is_some()
    }
}

pub struct Watch {
    pub video: Video,
    pub load: Load,
    pub info: Option<WatchInfo>,
    pub playlist: Option<Arc<nt_core::PlaylistPanel>>,
    pub playlist_more: More,
    pub playlist_end: Cell<f32>,
    pub related: Vec<Video>,
    pub related_selected: Option<usize>,
    initial_related: Option<nt_core::Page<Video>>,
    pub related_more: More,
    related_req: u64,
    related_next: Option<Continuation>,
    pub related_end: Cell<f32>,
    pub related_chip_offset: Cell<f32>,
    related_pending_chip: Option<usize>,
    pub comments: Vec<Comment>,
    /// Character counts, for height estimates before a comment has been laid out.
    pub comment_chars: Vec<u32>,
    pub comments_next: Option<Continuation>,
    /// A first comments page has arrived (it can hold no comments and still have a cursor).
    comments_started: bool,
    pub comments_total: Option<String>,
    pub comments_state: Comments,
    pub comments_req: u64,
    pub comments_sort: Vec<nt_core::CommentSort>,
    comments_pending_sort: Option<usize>,
    pub replies: HashMap<usize, Replies>,
    comments_revision: u64,
    pub comments_open: HashSet<usize>,
    pub comments_layout: RefCell<VarList>,
    pub desc_open: bool,
    /// The description split into plain runs and links, parsed once when it arrives.
    pub desc_spans: Vec<(std::ops::Range<usize>, Option<Link>)>,
    pub resume: f64,
    pub paused: bool,
    pub first_frame: Cell<bool>,
    /// Where the comments header sits in the content, as of the last build.
    pub comments_y: Cell<f32>,
    /// Where the description box sits in the content, as of the last build.
    pub desc_y: Cell<f32>,
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
    query: String,
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
    // Playback.
    pub play: PlaybackState,
    pub frame: Option<Arc<VideoFrame>>,
    frame_gen: u64,
    pub playing_page: Option<u64>,
    /// Where the current load starts; shown as the position until the player is ready.
    start_at: f64,
    /// A bounded cache of frames for history entries whose decoder was replaced.
    saved_frames: VecDeque<(u64, Arc<VideoFrame>)>,
    parked: Option<u64>,
    restore_pause: bool,
    /// This page's video has been seen playing since it was (re)started, so a paused state
    /// is the user's (or the media keys') and not the previous video's leftover.
    seen_playing: bool,
    pub want_playback_mark: Cell<bool>,
    pub surface_visible: Cell<bool>,
    surface_requested: bool,
    pub player_error: Option<String>,
    pub fullscreen: bool,
    pub fullscreen_scroll: ScrollModel,
    pub menu: Option<MenuPage>,
    pub volume_hover: bool,
    pub bezel: Option<(Icon, Instant)>,
    pub up_next: Option<UpNext>,
    up_seq: u64,
    pub card_menu: Option<CardMenu>,
    pub player_context: Option<Point>,
    pub stats: bool,
    pub shortcuts: bool,
    pub queue_open: bool,
    pub queue: Vec<Video>,
    playlist_req: u64,
    pub search_filters: bool,
    pub guide_overlay: bool,
    pub sleep: SleepTimer,
    sleep_seq: u64,
    /// The caption language last turned on, preferred the next time CC is pressed.
    captions_lang: Option<String>,
    test_step: u64,
    /// Test-only (`NT_TEST_CYCLE=controls`): keep the player controls shown.
    pub test_controls: bool,
    test_cc_done: bool,
    /// Captions' lift above the control bar: 0 = resting low, 1 = clear of the controls.
    caption_lift: f32,
    /// A running lift animation: (from, to, start).
    caption_anim: Option<(f32, f32, Instant)>,
    /// The `sub-pos` last sent to the player, so frames that change nothing send nothing.
    caption_pos: Cell<f64>,
    pub probe: crate::probe::Probe,
    pub scrub_preview: Option<ImageHandle>,
    scrub_frame: Option<nt_core::StoryboardFrame>,
    scrub_request: u64,
    pub toast: Option<(String, u64)>,
    toast_seq: u64,
    /// URL resolution is speculative. Only opening a Watch page starts the media.
    preload: Option<String>,
    pub resume_offer: Option<nt_core::LastPlayed>,
    pub preview: Option<PreviewTarget>,
    pub preview_frame: Option<Arc<VideoFrame>>,
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
            play: PlaybackState::default(),
            frame: None,
            frame_gen: 0,
            playing_page: None,
            start_at: 0.0,
            saved_frames: VecDeque::new(),
            parked: None,
            restore_pause: false,
            seen_playing: false,
            want_playback_mark: Cell::new(false),
            surface_visible: Cell::new(false),
            surface_requested: false,
            player_error: None,
            fullscreen: false,
            fullscreen_scroll: ScrollModel::default(),
            menu: None,
            volume_hover: false,
            bezel: None,
            up_next: None,
            up_seq: 0,
            card_menu: None,
            player_context: None,
            stats: false,
            shortcuts: false,
            queue_open: false,
            queue: library().queue(),
            playlist_req: 0,
            search_filters: false,
            guide_overlay: false,
            sleep: test_sleep_timer(),
            sleep_seq: 0,
            captions_lang: None,
            test_step: 0,
            test_controls: false,
            test_cc_done: false,
            caption_lift: 0.0,
            caption_anim: None,
            caption_pos: Cell::new(f64::NAN),
            probe: crate::probe::Probe::new(),
            scrub_preview: None,
            scrub_frame: None,
            scrub_request: 0,
            toast: None,
            toast_seq: 0,
            preload: None,
            preview: None,
            preview_frame: None,
            resume_offer: library().last_played().filter(|p| p.position > 0.0),
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

    pub fn forward_depth(&self) -> usize {
        self.forward.len()
    }

    pub fn guide_is_overlay(&self) -> bool {
        self.guide_overlay && !self.fullscreen
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
            query: self.query.clone(),
            body,
        }
    }

    fn page_mut(&mut self, id: u64) -> Option<&mut Page> {
        self.pages
            .iter_mut()
            .chain(self.forward.iter_mut())
            .find(|p| p.id == id)
    }

    // ---- navigation ---------------------------------------------------------------------

    fn leave_top(&mut self) {
        self.stop_preview();
        self.clear_scrub_preview();
        self.menu = None;
        self.card_menu = None;
        self.player_context = None;
        self.guide_overlay = false;
        self.search_filters = false;
        self.queue_open = false;
        self.up_next = None;
        self.volume_hover = false;
        let Some(page) = self.pages.last_mut() else {
            return;
        };
        page.query.clone_from(&self.query);
        if let Body::Watch(w) = &mut page.body
            && self.playing_page == Some(page.id)
        {
            // Only a settled state of this video says where it is, and only after it was seen
            // playing does `paused` mean someone paused it. Right after a load the state can
            // still carry the pause set when the previous page was left; rapid Back/Forward
            // would then park this page as paused at 0:00.
            let settled = matches!(self.play.status, Status::Ready | Status::Ended)
                && self.play.source.as_deref() == Some(w.video.id.as_str());
            w.resume = if settled {
                self.play.position
            } else {
                self.start_at
            };
            w.paused = if settled && self.seen_playing {
                self.play.paused
            } else {
                self.restore_pause
            };
            if self.play.duration > 0.0 && !self.play.is_live {
                self.prefs.record(&w.video.id, w.resume, self.play.duration);
                self.prefs.save();
            }
            if let Some(p) = ext::existing_player() {
                p.pause();
                self.parked = Some(page.id);
            }
            self.playing_page = None;
            if let Some(frame) = self.frame.take() {
                self.saved_frames.retain(|(id, _)| *id != page.id);
                self.saved_frames.push_back((page.id, frame));
                if self.saved_frames.len() > 4 {
                    self.saved_frames.pop_front();
                }
            }
            self.want_playback_mark.set(false);
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
                chips: Vec::new(),
                show_initial: true,
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
        }
        self.arrive();
        crate::scroller::trace_event("back");
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
        crate::scroller::trace_event("forward");
        Task::none()
    }

    /// A page came back on top (Back or Forward): resume its video, refresh library lists.
    fn arrive(&mut self) {
        let Some(top) = self.top() else { return };
        let id = top.id;
        self.query = top.query.clone();
        let Some(top) = self.top() else { return };
        match &top.body {
            Body::Watch(w) => {
                let (vid, resume, paused) = (w.video.id.clone(), w.resume, w.paused);
                self.start_playback(id, &vid, resume, paused);
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
                if h.show_initial {
                    return Task::perform(client().home_feed(), move |r| {
                        Message::HomeLoaded(id, req, r)
                    });
                }
                let job = if let Some(chip) = h.chips.get(h.topic) {
                    match &chip.next {
                        Some(next) => client().more_videos(next),
                        None => {
                            return Task::perform(client().home_feed(), move |r| {
                                Message::HomeLoaded(id, req, r)
                            });
                        }
                    }
                } else {
                    client().home(TOPICS.get(h.topic).map_or("all", |t| t.id))
                };
                Task::perform(job, move |result| Message::FeedLoaded {
                    page: id,
                    req,
                    append: false,
                    result,
                })
            }
            Body::Search(s) => Self::load_search(id, s),
            Body::Watch(w) => {
                w.load = Load::Loading;
                let job = match &w.playlist {
                    Some(list) => client().watch_in_playlist(&w.video.id, &list.id),
                    None => client().watch(&w.video.id),
                };
                Task::perform(job, move |r| Message::WatchLoaded(id, Box::new(r)))
            }
            Body::Channel(c) => {
                c.load = Load::Loading;
                c.req += 1;
                let req = c.req;
                Task::perform(client().channel_tab(&c.id, c.tab), move |r| {
                    Message::ChannelLoaded(id, req, Box::new(r))
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

    /// Where a video opens: a transcript match's moment, else the saved resume point.
    fn load_search(page: u64, search: &mut Search) -> Task<Message> {
        search.load = Load::Loading;
        search.req += 1;
        let req = search.req;
        Task::perform(
            client().search_filtered(&search.query, &search.filter),
            move |r| Message::SearchLoaded(page, req, false, Box::new(r)),
        )
    }

    fn start_time(&self, v: &Video) -> f64 {
        v.moment
            .as_ref()
            .map_or_else(|| self.prefs.resume_at(&v.id), |m| m.start_secs.max(0.0))
    }

    fn open_video(&mut self, video: Video) -> Task<Message> {
        let resume = self.start_time(&video);
        self.open_video_at(video, resume)
    }

    fn open_video_at(&mut self, video: Video, resume: f64) -> Task<Message> {
        self.open_watch(video, resume, None)
    }

    fn open_watch(
        &mut self,
        video: Video,
        resume: f64,
        playlist: Option<Arc<nt_core::PlaylistPanel>>,
    ) -> Task<Message> {
        // The GPU surface must exist before the first load. Attach it now and open once
        // that's done (about 60 ms, only when no hover or press got there first).
        if let Some(attach) = self.surface_needed() {
            // Resolve the streams meanwhile; the load after the attach adopts them.
            if let Some(p) = ext::existing_player() {
                p.prepare_next(Some(&video.id));
            }
            return attach.chain(Task::done(Message::OpenWatch(Box::new((
                video, resume, playlist,
            )))));
        }
        crate::scroller::trace_event("open_video");
        self.resume_offer = None;
        library().record_watch(&video);
        let vid = video.id.clone();
        let watch = Watch {
            video,
            load: Load::Loading,
            info: None,
            playlist,
            playlist_more: More::Idle,
            playlist_end: Cell::new(f32::INFINITY),
            related: Vec::new(),
            related_selected: None,
            initial_related: None,
            related_more: More::Idle,
            related_req: 0,
            related_next: None,
            related_end: Cell::new(f32::INFINITY),
            related_chip_offset: Cell::new(0.0),
            related_pending_chip: None,
            comments: Vec::new(),
            comment_chars: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: Comments::NotStarted,
            comments_started: false,
            comments_req: 0,
            comments_sort: Vec::new(),
            comments_pending_sort: None,
            replies: HashMap::new(),
            comments_revision: 0,
            comments_open: HashSet::new(),
            comments_layout: RefCell::new(VarList::default()),
            desc_open: false,
            desc_spans: Vec::new(),
            resume,
            paused: false,
            first_frame: Cell::new(false),
            comments_y: Cell::new(f32::INFINITY),
            desc_y: Cell::new(f32::INFINITY),
        };
        let id = self.push(Body::Watch(Box::new(watch)));
        self.start_playback(id, &vid, resume, false);
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
            tab: nt_core::ChannelTab::Videos,
            sort: None,
            sort_pending: None,
            req: 0,
            about: None,
            about_load: Load::Ready,
            playlists_more: More::Idle,
        }));
        self.load_top()
    }

    fn submit_search(&mut self, query: String) -> Task<Message> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Task::none();
        }
        self.search_focused = false;
        self.search_open = false;
        self.push(Body::Search(Search {
            query: query.clone(),
            load: Load::Loading,
            feed: Feed::default(),
            notices: Vec::new(),
            moments: Feed::default(),
            moments_total: None,
            chips: Vec::new(),
            filters: Vec::new(),
            filter: SearchFilter::All,
            entries: Vec::new(),
            req: 0,
        }));
        self.query = query;
        Task::batch([unfocus(), self.load_top()])
    }

    pub fn list(&self, which: ListRef) -> &[Video] {
        match (self.top().map(|p| &p.body), which) {
            (Some(Body::Home(h)), ListRef::Main) => &h.feed.items,
            (Some(Body::Search(s)), ListRef::Main) => &s.feed.items,
            (Some(Body::Search(s)), ListRef::Moments) => &s.moments.items,
            (Some(Body::Channel(c)), ListRef::Main) => &c.feed.items,
            (Some(Body::Subs(s)), ListRef::Main) => &s.feed.items,
            (Some(Body::History(v)), ListRef::Main) | (Some(Body::Later(v)), ListRef::Main) => v,
            (Some(Body::Watch(w)), ListRef::Main | ListRef::Related) => &w.related,
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

    fn start_playback(&mut self, page: u64, video_id: &str, resume: f64, paused: bool) {
        self.frame = self
            .saved_frames
            .iter()
            .find(|(id, _)| *id == page)
            .map(|(_, f)| f.clone());
        self.clear_scrub_preview();
        self.up_next = None;
        self.menu = None;
        self.playing_page = Some(page);
        self.play = PlaybackState::default();
        self.player_error = None;
        let p = match ext::player() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("nt-iced: player: {e}");
                self.player_error = Some("The video player couldn't start.".into());
                return;
            }
        };
        let parked = self.parked.take() == Some(page)
            && p.state().source.as_deref() == Some(video_id)
            && !matches!(p.state().status, Status::Idle | Status::Failed(_));
        self.preload = None;
        // The core starts the stream at the saved offset and adopts matching preparation.
        let geo = crate::view::watch_geo(self);
        p.set_target_size(
            (geo.player_w * self.scale) as u32,
            (geo.player_h * self.scale) as u32,
        );
        if !parked && let Err(e) = p.load_at(video_id, resume) {
            self.player_error = Some(friendly(&e));
            return;
        }
        self.start_at = resume;
        self.seen_playing = false;
        self.restore_pause = paused;
        if paused {
            p.pause();
        } else if parked && p.state().status != Status::Ended {
            p.play();
        }
        self.play = normalized_playback(p.state());
        if let Status::Failed(error) = &self.play.status {
            eprintln!("nt-iced: playback: {error}");
        }
        if !matches!(self.play.status, Status::Ready | Status::Ended) {
            self.play.position = resume;
        }
        self.want_playback_mark.set(true);
        self.frame = p.frame().or(self.frame.take());
        self.frame_gen = u64::MAX;
        ext::reset_gate();
        ext::refresh_player();
        if parked {
            self.prepare_successor();
        }
    }

    pub fn can_preload(&self) -> bool {
        self.playing_page.is_none()
            && self.parked.is_none()
            && !self.fullscreen
            && self.card_menu.is_none()
            && !self.sugg_open
            && !self.shortcuts
            && !self.search_filters
            && !self.guide_is_overlay()
            && self.top().is_some_and(|p| !p.scroll.is_moving())
    }

    fn preload(&mut self, list: ListRef, index: usize, hash: u64) -> Task<Message> {
        if !self.can_preload() {
            return Task::none();
        }
        let Some(video) = self
            .list(list)
            .get(index)
            .filter(|v| id_hash(&v.id) == hash)
        else {
            return Task::none();
        };
        if self.preload.as_ref() == Some(&video.id) {
            return Task::none();
        }
        let id = video.id.clone();
        let Ok(p) = ext::player() else {
            return Task::none();
        };
        let geo = crate::view::watch_geo(self);
        p.set_target_size(
            (geo.player_w * self.scale) as u32,
            (geo.player_h * self.scale) as u32,
        );
        p.prepare_next(Some(&id));
        crate::scroller::trace_event("preload");
        self.preload_seq += 1;
        let seq = self.preload_seq;
        self.preload = Some(id);
        let attach = self.surface_needed().unwrap_or_else(Task::none);
        Task::batch([
            attach,
            Task::future(async move {
                tokio::time::sleep(Duration::from_millis(2500)).await;
                Message::PreloadExpire(seq)
            }),
        ])
    }

    /// The video the open card menu is for, if it is still where it was; closes the menu.
    fn menu_video(&mut self) -> Option<&Video> {
        let m = self.card_menu.take()?;
        self.list(m.list)
            .get(m.index)
            .filter(|v| id_hash(&v.id) == m.hash)
    }

    pub fn next_video(&self) -> Option<&Video> {
        self.queue.first().or_else(|| {
            let watch = self.watch()?;
            watch
                .playlist
                .as_ref()
                .and_then(|list| list.next_video())
                .or_else(|| watch.related.first())
        })
    }

    fn play_playlist_row(&mut self, index: usize) -> Task<Message> {
        let Some(list) = self.watch().and_then(|w| w.playlist.as_ref()) else {
            return Task::none();
        };
        let Some(video) = list.items.get(index).cloned() else {
            return Task::none();
        };
        let mut list = list.clone();
        Arc::make_mut(&mut list).index = Some(index);
        let resume = self.start_time(&video);
        self.open_watch(video, resume, Some(list))
    }

    fn update_queue(&mut self) {
        self.queue = library().queue();
        self.prepare_successor();
    }

    fn stop_preview(&mut self) {
        if self.preview.take().is_some() {
            self.preview_frame = None;
            ext::stop_preview();
        }
    }

    fn preview_video(&self, target: PreviewTarget) -> Option<&Video> {
        if self.top_id() != target.page || self.top().is_none_or(|p| p.scroll.is_moving()) {
            return None;
        }
        self.list(target.list)
            .get(target.index)
            .filter(|v| id_hash(&v.id) == target.hash && v.upcoming.is_none())
    }

    fn prepare_successor(&self) {
        if self.playing_page != Some(self.top_id()) {
            return;
        }
        let Some(p) = ext::existing_player() else {
            return;
        };
        let candidate = self.next_video();
        let automatic =
            self.prefs.autoplay && !self.play.looping && self.sleep != SleepTimer::EndOfVideo;
        p.prepare_next(candidate.filter(|_| automatic).map(|v| v.id.as_str()));
    }

    /// The attach task when the GPU surface is wanted and not yet asked for. Creates the player
    /// if the warm-up hasn't, since the surface belongs to it.
    fn surface_needed(&mut self) -> Option<Task<Message>> {
        if !crate::surface::enabled() || self.surface_requested || self.window.is_none() {
            return None;
        }
        ext::player().ok()?;
        Some(self.init_surface())
    }

    fn init_surface(&mut self) -> Task<Message> {
        let Some(id) = self
            .window
            .filter(|_| !self.surface_requested && ext::existing_player().is_some())
        else {
            return Task::none();
        };
        self.surface_requested = true;
        crate::surface::attach(id, self.scale)
    }

    fn on_player(&mut self) -> Task<Message> {
        ext::player_taken();
        let Some(p) = ext::existing_player() else {
            return Task::none();
        };
        let before = self.play.status.clone();
        let Some(w) = self.watch().filter(|_| self.playing_page.is_some()) else {
            return Task::none();
        };
        let state = normalized_playback(p.state());
        if state.source.as_deref() != Some(w.video.id.as_str()) {
            return Task::none();
        }
        if let Status::Failed(error) = &state.status
            && self.play.status != state.status
        {
            eprintln!("nt-iced: playback: {error}");
        }
        self.play = state;
        if self.play.status == Status::Ready && !self.play.paused {
            self.seen_playing = true;
        }
        if matches!(self.play.status, Status::Idle | Status::Loading) {
            self.play.position = self.start_at;
        }
        let generation = p.frame_generation();
        if generation != self.frame_gen
            && let Some(frame) = p.frame()
        {
            self.frame_gen = generation;
            self.frame = Some(frame);
        }
        if self.restore_pause && self.play.status == Status::Ready {
            p.pause();
            self.play.paused = true;
            self.seen_playing = true;
            self.restore_pause = false;
        }
        if self.play.status == Status::Ready && before != Status::Ready {
            crate::scroller::trace_event("player_ready");
            self.prepare_successor();
            self.caption_pos.set(f64::NAN);
            self.place_captions();
            // Test-only switches for input-free caption checks.
            let fullscreen = if test_flag("NT_TEST_FULLSCREEN") {
                self.set_fullscreen(true)
            } else {
                Task::none()
            };
            return fullscreen;
        }
        // Test-only: turn captions on once this video's tracks are known.
        if test_flag("NT_TEST_CC")
            && self.play.status == Status::Ready
            && !self.play.captions.is_empty()
            && self.play.selected_captions.is_none()
            && !self.test_cc_done
        {
            self.test_cc_done = true;
            return self.toggle_captions();
        }
        if self.play.status == Status::Ended && before != Status::Ended {
            crate::scroller::trace_event("playback_ended");
            return self.video_ended();
        }
        Task::none()
    }

    fn video_ended(&mut self) -> Task<Message> {
        if self.sleep == SleepTimer::EndOfVideo {
            return self.fire_sleep();
        }
        if self.play.looping {
            return Task::none();
        }
        let Some(page) = self.playing_page else {
            return Task::none();
        };
        if let Some(w) = self.watch() {
            let id = w.video.id.clone();
            self.prefs
                .record(&id, self.play.duration, self.play.duration);
            self.prefs.save();
        }
        if self.next_video().is_none() {
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

    fn fire_sleep(&mut self) -> Task<Message> {
        crate::scroller::trace_event("sleep_timer_fired");
        self.sleep = SleepTimer::Off;
        self.sleep_seq += 1;
        self.up_next = None;
        self.player_do(|p| p.pause());
        self.toast("Sleep timer ended".into())
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

    /// Turn captions on or off (CC button, C key): the language used last, else the first
    /// track made by people, else the first.
    fn toggle_captions(&mut self) -> Task<Message> {
        let tracks = &self.play.captions;
        if tracks.is_empty() {
            return Task::none();
        }
        if self.play.selected_captions.is_some() {
            return self.set_captions(None);
        }
        let index = self
            .captions_lang
            .as_ref()
            .and_then(|lang| tracks.iter().position(|t| &t.language_code == lang))
            .or_else(|| tracks.iter().position(|t| !t.auto_generated))
            .unwrap_or(0);
        self.set_captions(Some((index, None)))
    }

    fn set_captions(&mut self, selection: Option<(usize, Option<String>)>) -> Task<Message> {
        let Some(p) = ext::existing_player().filter(|_| self.playing_page.is_some()) else {
            return Task::none();
        };
        if let Some((i, _)) = &selection
            && let Some(track) = self.play.captions.get(*i)
        {
            self.captions_lang = Some(track.language_code.clone());
        }
        self.bezel(if selection.is_some() {
            Icon::ClosedCaptionFill
        } else {
            Icon::ClosedCaption
        });
        // Shown at once; the state from the player confirms it when the track is loaded.
        self.play.selected_captions = selection.clone();
        self.caption_pos.set(f64::NAN);
        self.place_captions();
        let page = self.top_id();
        Task::perform(p.set_captions(selection), move |r| {
            Message::PlayerSettingDone(page, r)
        })
    }

    fn clear_scrub_preview(&mut self) {
        self.scrub_frame = None;
        self.scrub_preview = None;
        self.scrub_request += 1;
    }

    fn scrub_hover(&mut self, seconds: Option<f64>) -> Task<Message> {
        let (Some(seconds), Some(board)) = (seconds, &self.play.storyboard) else {
            self.clear_scrub_preview();
            return Task::none();
        };
        let width = (160.0 * self.scale).ceil() as u32;
        let frame = board.level_for_width(width).frame_at(seconds);
        if self.scrub_frame.as_ref() == Some(&frame) {
            return Task::none();
        }
        self.scrub_frame = Some(frame);
        self.scrub_request += 1;
        let req = self.scrub_request;
        Task::perform(board.preview(seconds, width), move |r| {
            Message::ScrubImage(req, r)
        })
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
            Body::Search(s) if s.load == Load::Ready => {
                if s.feed.next.is_none() && s.moments.next.is_some() {
                    Some((&mut s.moments, s.req + SEARCH_MOMENTS))
                } else {
                    Some((&mut s.feed, SEARCH_RESULTS))
                }
            }
            Body::Channel(c)
                if c.load == Load::Ready && c.tab == nt_core::ChannelTab::Playlists =>
            {
                return if near && c.playlists_more == More::Idle {
                    Self::load_channel_playlists(id, c)
                } else {
                    Task::none()
                };
            }
            Body::Channel(c) if c.load == Load::Ready => Some((&mut c.feed, c.req)),
            Body::Watch(w) => {
                let near_comments = w.comments_y.get() <= offset + 2.5 * vh;
                let go = match w.comments_state {
                    Comments::NotStarted => near_comments && w.load == Load::Ready,
                    Comments::Idle => near && w.comments_next.is_some(),
                    _ => false,
                };
                let related =
                    if w.related_more == More::Idle && w.related_end.get() <= offset + 2.5 * vh {
                        Self::load_related(id, w, None)
                    } else {
                        Task::none()
                    };
                let comments = if go {
                    Self::load_comments(id, w)
                } else {
                    Task::none()
                };
                let playlist = if w.playlist_more == More::Idle
                    && (w.playlist_end.get() <= offset + 2.5 * vh
                        || w.playlist.as_ref().is_some_and(|p| {
                            p.index
                                .is_some_and(|i| p.items.len().saturating_sub(i) <= 10)
                        })) {
                    Self::load_playlist_more(id, w)
                } else {
                    Task::none()
                };
                return Task::batch([related, comments, playlist]);
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
        if let Body::Search(s) = &page.body
            && req == SEARCH_RESULTS
        {
            let seq = s.req;
            return Task::perform(client().more_search(&next), move |r| {
                Message::SearchLoaded(id, seq, true, Box::new(r))
            });
        }
        Task::perform(client().more_videos(&next), move |result| {
            Message::FeedLoaded {
                page: id,
                req,
                append: true,
                result,
            }
        })
    }

    fn load_playlist_more(page: u64, watch: &mut Watch) -> Task<Message> {
        if watch.playlist_more == More::Loading {
            return Task::none();
        }
        let Some(cursor) = watch.playlist.as_ref().and_then(|p| p.next.as_ref()) else {
            return Task::none();
        };
        watch.playlist_more = More::Loading;
        Task::perform(client().more_playlist_videos(cursor), move |r| {
            Message::PlaylistMoreLoaded(page, r)
        })
    }

    fn load_channel_playlists(page: u64, channel: &mut Channel) -> Task<Message> {
        let Some(cursor) = channel
            .info
            .as_ref()
            .and_then(|info| info.playlists.next.as_ref())
        else {
            return Task::none();
        };
        channel.playlists_more = More::Loading;
        let req = channel.req;
        Task::perform(client().more_playlists(cursor), move |r| {
            Message::ChannelPlaylistsLoaded(page, req, r)
        })
    }

    fn load_comments(id: u64, w: &mut Watch) -> Task<Message> {
        if w.comments_state == Comments::Loading {
            return Task::none();
        }
        // The watch page's cursor is for page 1 only; after that, always the returned one.
        let next = if w.comments_started {
            w.comments_next.clone()
        } else {
            w.info.as_ref().and_then(|i| i.comments.clone())
        };
        let Some(next) = next else {
            return Task::none();
        };
        w.comments_state = Comments::Loading;
        w.comments_req += 1;
        let req = w.comments_req;
        Task::perform(client().more_comments(&next), move |r| {
            Message::CommentsLoaded(id, req, None, r)
        })
    }

    fn load_related(page: u64, watch: &mut Watch, chip: Option<usize>) -> Task<Message> {
        let cursor = match chip {
            Some(index) => watch
                .info
                .as_ref()
                .and_then(|info| info.related_chips.get(index))
                .and_then(|chip| chip.next.clone()),
            None if watch.related_more != More::Loading => watch.related_next.clone(),
            None => None,
        };
        let Some(cursor) = cursor else {
            return Task::none();
        };
        watch.related_pending_chip = chip;
        watch.related_req += 1;
        let req = watch.related_req;
        watch.related_more = More::Loading;
        Task::perform(client().more_videos(&cursor), move |r| {
            Message::RelatedLoaded(page, req, chip, r)
        })
    }

    fn load_replies(&mut self, index: usize, toggle: bool) -> Task<Message> {
        let page = self.top_id();
        let Some(Page {
            body: Body::Watch(watch),
            scroll,
            ..
        }) = self.top_mut()
        else {
            return Task::none();
        };
        let Some(first) = watch
            .comments
            .get(index)
            .and_then(|comment| comment.replies.clone())
        else {
            return Task::none();
        };
        let reply = watch.replies.entry(index).or_default();
        if toggle {
            reply.open = !reply.open;
            scroll.forget_height(crate::view::comment_key(index));
            if !reply.open || !reply.items.is_empty() {
                return Task::none();
            }
        }
        if reply.more == Some(More::Loading) {
            return Task::none();
        }
        let cursor = if reply.items.is_empty() {
            Some(first)
        } else {
            reply.next.clone()
        };
        let Some(cursor) = cursor else {
            return Task::none();
        };
        reply.more = Some(More::Loading);
        let revision = watch.comments_revision;
        Task::perform(client().more_replies(&cursor), move |r| {
            Message::RepliesLoaded(page, revision, index, r)
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

    fn sync_player_geometry(&self) {
        let geo = crate::view::watch_geo(self);
        self.player_do(|p| {
            p.set_target_size(
                (geo.player_w * self.scale) as u32,
                (geo.player_h * self.scale) as u32,
            )
        });
        self.prepare_successor();
        self.place_captions();
    }

    /// mpv `sub-pos` for the current lift. YouTube keeps captions about 8% above the picture's
    /// bottom and lifts them clear of the progress and control bars while those show. The
    /// bars are anchored to the player box, the captions to the picture, so letterboxing in
    /// theatre and fullscreen reduces how far they need to move.
    fn caption_percent(&self) -> f64 {
        // With sub-pos 100 the core's `sub-margin-y` (44 of 720) leaves 6.1% below the block.
        const REST: f64 = 98.0;
        // Bar row 48 + scrubber 16 + 12 px of air, in logical px from the box's bottom.
        const CONTROLS: f32 = 76.0;
        let geo = crate::view::watch_geo(self);
        let aspect = self.play.aspect().unwrap_or(16.0 / 9.0);
        let (_, vh) = crate::surface::fit(geo.player_w, geo.player_h, aspect);
        let gap = ((geo.player_h - vh) / 2.0).max(0.0);
        let needed = f64::from(((CONTROLS - gap) / vh.max(1.0)).max(0.0)) * 100.0;
        let lifted = (100.0 - (needed - 100.0 * 44.0 / 720.0)).min(REST);
        REST + (lifted - REST) * f64::from(self.caption_lift)
    }

    fn place_captions(&self) {
        if self.play.selected_captions.is_none() {
            return;
        }
        let pos = self.caption_percent();
        if (pos - self.caption_pos.get()).abs() >= 0.05 || self.caption_pos.get().is_nan() {
            self.caption_pos.set(pos);
            self.player_do(|p| p.set_caption_position(pos));
        }
    }

    fn set_fullscreen(&mut self, on: bool) -> Task<Message> {
        if self.fullscreen == on || (on && self.watch().is_none()) {
            return Task::none();
        }
        self.fullscreen = on;
        if on {
            self.fullscreen_scroll.reset();
        }
        self.sync_player_geometry();
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
        self.stop_preview();
        crate::surface::detach();
        // `stop` records the resume checkpoint through the library's worker; the process may
        // end before that write runs, so write it now (B13 `Library::flush`).
        library().flush();
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
            // The GPU surface (EGL and the GL drivers, ~40 MiB) waits for the first sign that
            // a video will play: a hover or press on a card, or an open.
            Message::Ext(Ext::PlayerReady) => Task::none(),
            Message::SurfaceAttached => Task::none(),
            Message::OpenWatch(open) => {
                let (video, resume, playlist) = *open;
                self.open_watch(video, resume, playlist)
            }
            Message::ScrollMoved => {
                self.stop_preview();
                self.maybe_more()
            }
            Message::Topic(i) => {
                let Some(Page {
                    body: Body::Home(h),
                    scroll,
                    ..
                }) = self.top_mut()
                else {
                    return Task::none();
                };
                if i >= (if h.chips.is_empty() {
                    TOPICS.len()
                } else {
                    h.chips.len()
                }) || (h.topic == i && h.load == Load::Ready)
                {
                    return Task::none();
                }
                h.topic = i;
                h.show_initial = h.chips.get(i).map_or(i == 0, |chip| chip.next.is_none());
                scroll.reset();
                self.load_top()
            }
            Message::HomeLoaded(page, req, result) => {
                let prefs = &self.prefs;
                let result = result.map(|mut home| {
                    home.videos
                        .items
                        .retain(|v| !prefs.is_hidden(&v.id) && !library().is_hidden(v));
                    home
                });
                let Some(Page {
                    body: Body::Home(h),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != h.req {
                    return Task::none();
                }
                match result {
                    Ok(home) => {
                        h.topic = home.chips.iter().position(|c| c.selected).unwrap_or(0);
                        h.chips = home.chips;
                        h.feed.set(home.videos);
                        h.load = Load::Ready;
                        mark_on_next_frame(marks::HOME_LOADED);
                    }
                    Err(e) => h.load = Load::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                Task::none()
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
            Message::PreviewEnter(target) => {
                if self.preview == Some(target)
                    || self.preview_video(target).is_none()
                    || self.card_menu.is_some()
                    || self.sugg_open
                    || self.guide_is_overlay()
                    || self.queue_open
                    || self.shortcuts
                    || self.search_filters
                {
                    return Task::none();
                }
                self.stop_preview();
                self.preview = Some(target);
                Task::future(async move {
                    let result = tokio::task::spawn_blocking(ext::init_preview)
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                    Message::PreviewReady(target, result)
                })
            }
            Message::PreviewReady(target, result) => {
                if self.preview == Some(target) {
                    if result.is_ok()
                        && let Some(video) = self.preview_video(target)
                    {
                        ext::play_preview(&video.id, target.width, target.height);
                    } else {
                        self.stop_preview();
                    }
                }
                Task::none()
            }
            Message::PreviewLeave(target) => {
                if self.preview == Some(target) {
                    self.stop_preview();
                }
                Task::none()
            }
            Message::StopPreview => {
                self.stop_preview();
                Task::none()
            }
            Message::Ext(Ext::Preview) => {
                if let Some(target) = self.preview
                    && let Some(video) = self.preview_video(target)
                {
                    self.preview_frame = ext::preview_frame(&video.id);
                } else {
                    self.stop_preview();
                }
                Task::none()
            }
            Message::PreloadExpire(seq) => {
                if seq == self.preload_seq
                    && self.preload.take().is_some()
                    && self.playing_page.is_none()
                    && let Some(p) = ext::existing_player()
                {
                    p.prepare_next(None);
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
            Message::OpenPlaylist(id) => {
                self.playlist_req += 1;
                let (page, req) = (self.top_id(), self.playlist_req);
                Task::perform(client().playlist(&id), move |r| {
                    Message::PlaylistLoaded(page, req, r)
                })
            }
            Message::PlaylistLoaded(page, req, result) => {
                if page != self.top_id() || req != self.playlist_req {
                    return Task::none();
                }
                match result {
                    Ok(panel) => {
                        let Some(video) = panel.items.get(panel.index.unwrap_or(0)).cloned() else {
                            return self.toast("This playlist has no playable videos.".into());
                        };
                        let resume = self.start_time(&video);
                        self.open_watch(video, resume, Some(Arc::new(panel)))
                    }
                    Err(e) => self.toast(friendly(&e)),
                }
            }
            Message::PlaylistPlay(index) => self.play_playlist_row(index),
            Message::PreviousVideo => {
                let Some(index) = self
                    .watch()
                    .and_then(|w| w.playlist.as_ref())
                    .and_then(|p| p.index)
                    .and_then(|i| i.checked_sub(1))
                else {
                    return Task::none();
                };
                self.play_playlist_row(index)
            }
            Message::PlaylistMore => {
                let page = self.top_id();
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) => Self::load_playlist_more(page, w),
                    _ => Task::none(),
                }
            }
            Message::PlaylistMoreLoaded(page, result) => {
                let Some(Page {
                    body: Body::Watch(w),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                let Some(list) = &mut w.playlist else {
                    return Task::none();
                };
                match result {
                    Ok(batch) => {
                        let list = Arc::make_mut(list);
                        list.items.extend(batch.items);
                        list.next = batch.next;
                        w.playlist_more = More::Idle;
                    }
                    Err(e) => w.playlist_more = More::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                if self.top_id() == page {
                    self.prepare_successor();
                }
                Task::none()
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
                    pg.items
                        .retain(|v| !prefs.is_hidden(&v.id) && !library().is_hidden(v));
                    pg
                });
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let (load, feed, mark) = match &mut p.body {
                    Body::Home(h) if h.req == req => (&mut h.load, &mut h.feed, marks::HOME_LOADED),
                    Body::Search(s) if req == s.req + SEARCH_MOMENTS => {
                        (&mut s.load, &mut s.moments, marks::SEARCH_LOADED)
                    }
                    Body::Channel(c) if c.req == req => {
                        (&mut c.load, &mut c.feed, marks::CHANNEL_LOADED)
                    }
                    _ => return Task::none(),
                };
                match (result, append) {
                    (Ok(pg), false) => {
                        feed.set(pg);
                        *load = Load::Ready;
                        mark_on_next_frame(mark);
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
            Message::SearchFilters(open) => {
                self.close_suggestions();
                self.search_filters = open;
                Task::none()
            }
            Message::ApplySearchFilter(filter) => {
                self.search_filters = false;
                let page = self.top_id();
                let Some(Body::Search(s)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                if filter == s.filter && s.load == Load::Ready {
                    return Task::none();
                }
                s.filter = filter;
                Self::load_search(page, s)
            }
            Message::SearchLoaded(page, req, append, mut r) => {
                if let Ok(sp) = r.as_mut() {
                    sp.results
                        .items
                        .retain(|v| !self.prefs.is_hidden(&v.id) && !library().is_hidden(v));
                    sp.transcripts
                        .items
                        .retain(|v| !self.prefs.is_hidden(&v.id) && !library().is_hidden(v));
                }
                let Some(Page {
                    body: Body::Search(s),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != s.req {
                    return Task::none();
                }
                match *r {
                    Ok(mut sp) => {
                        if !sp.chips.is_empty() {
                            s.chips = sp.chips;
                        }
                        if !sp.filters.is_empty() {
                            s.filters = sp.filters;
                        }
                        if append {
                            s.feed.append(sp.results);
                            mark_on_next_frame(marks::MORE_LOADED);
                        } else {
                            s.feed.set(sp.results);
                            s.moments.set(sp.transcripts);
                            s.notices = sp.notices;
                            s.moments_total = sp.transcript_total;
                            s.entries.clear();
                            s.load = Load::Ready;
                            scroll.reset();
                            mark_on_next_frame(marks::SEARCH_LOADED);
                        }
                        if !sp.entries.is_empty() {
                            let indexes = s
                                .feed
                                .items
                                .iter()
                                .enumerate()
                                .map(|(i, v)| (v.id.clone(), i))
                                .collect();
                            s.add_entries(std::mem::take(&mut sp.entries), &indexes);
                        }
                    }
                    Err(e) if append => s.feed.more = Some(More::Failed(friendly(&e))),
                    Err(e) => s.load = Load::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                if self.top_id() == page {
                    self.prepare_successor();
                }
                Task::none()
            }
            Message::WatchLoaded(page, r) => {
                let Some(p) = self.page_mut(page) else {
                    return Task::none();
                };
                let Body::Watch(w) = &mut p.body else {
                    return Task::none();
                };
                match *r {
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
                        if let Some(playlist) = info.playlist.take() {
                            w.playlist = Some(Arc::new(playlist));
                        }
                        w.related = std::mem::take(&mut info.related.items);
                        w.related_next = info.related.next.take();
                        w.related_selected = info.related_chips.iter().position(|c| c.selected);
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
                    if self.play.status == Status::Ready {
                        self.prepare_successor();
                    }
                    return self.maybe_more();
                }
                Task::none()
            }
            Message::RelatedChip(index) => {
                let page = self.top_id();
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) if w.related_selected != Some(index) => {
                        let restore = w
                            .info
                            .as_ref()
                            .and_then(|info| info.related_chips.get(index))
                            .is_some_and(|chip| chip.selected && chip.next.is_none());
                        if restore && let Some(initial) = w.initial_related.take() {
                            w.related = initial.items;
                            w.related_next = initial.next;
                            w.related_selected = Some(index);
                            w.related_req += 1;
                            w.related_more = More::Idle;
                            self.prepare_successor();
                            Task::none()
                        } else {
                            Self::load_related(page, w, Some(index))
                        }
                    }
                    _ => Task::none(),
                }
            }
            Message::RelatedMore => {
                let page = self.top_id();
                let Some(Body::Watch(w)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                Self::load_related(page, w, w.related_pending_chip)
            }
            Message::RelatedLoaded(page, req, chip, result) => {
                let result = result.map(|mut p| {
                    p.items
                        .retain(|v| !self.prefs.is_hidden(&v.id) && !library().is_hidden(v));
                    p
                });
                let Some(Page {
                    body: Body::Watch(w),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != w.related_req {
                    return Task::none();
                }
                match result {
                    Ok(p) => {
                        if chip.is_some() {
                            if w.initial_related.is_none() {
                                w.initial_related = Some(nt_core::Page {
                                    items: std::mem::take(&mut w.related),
                                    next: w.related_next.take(),
                                });
                            } else {
                                w.related.clear();
                            }
                            w.related_selected = chip;
                        }
                        let mut ids: HashSet<_> = w.related.iter().map(|v| v.id.clone()).collect();
                        w.related
                            .extend(p.items.into_iter().filter(|v| ids.insert(v.id.clone())));
                        w.related_next = p.next;
                        w.related_more = More::Idle;
                        w.related_pending_chip = None;
                    }
                    Err(e) => w.related_more = More::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                Task::none()
            }
            Message::SortComments(index) => {
                let page = self.top_id();
                let Some(Body::Watch(w)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                let Some(sort) = w.comments_sort.get(index).filter(|s| !s.selected) else {
                    return Task::none();
                };
                w.comments_pending_sort = Some(index);
                w.comments_req += 1;
                w.comments_state = Comments::Loading;
                let req = w.comments_req;
                Task::perform(client().more_comments(&sort.next), move |r| {
                    Message::CommentsLoaded(page, req, Some(index), r)
                })
            }
            Message::ReplyToggle(index) => self.load_replies(index, true),
            Message::ReplyMore(index) => self.load_replies(index, false),
            Message::RepliesLoaded(page, revision, index, result) => {
                let Some(Page {
                    body: Body::Watch(w),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                let Some(reply) = w
                    .replies
                    .get_mut(&index)
                    .filter(|_| revision == w.comments_revision)
                else {
                    return Task::none();
                };
                match result {
                    Ok(p) => {
                        reply.items.extend(p.items);
                        reply.next = p.next;
                        reply.more = Some(More::Idle);
                    }
                    Err(e) => reply.more = Some(More::Failed(friendly(&e))),
                }
                scroll.forget_height(crate::view::comment_key(index));
                Task::none()
            }
            Message::CommentsLoaded(page, req, sort, r) => {
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
                        if let Some(sort) = sort {
                            for (index, option) in w.comments_sort.iter_mut().enumerate() {
                                option.selected = index == sort;
                            }
                            for index in 0..w.comments.len() {
                                p.scroll.forget_height(crate::view::comment_key(index));
                            }
                            w.comments_revision += 1;
                            w.comments.clear();
                            w.comment_chars.clear();
                            w.comments_open.clear();
                            w.replies.clear();
                        }
                        if !cp.sort.is_empty() {
                            w.comments_sort = cp.sort;
                        }
                        if cp.total.is_some() {
                            w.comments_total = cp.total;
                        }
                        w.comment_chars.extend(
                            cp.items
                                .iter()
                                .map(|c| c.text.chars().count().min(u32::MAX as usize) as u32),
                        );
                        w.comments.extend(cp.items);
                        w.comments_pending_sort = None;
                        w.comments_started = true;
                        w.comments_next = cp.next;
                        w.comments_state = Comments::Idle;
                        mark_on_next_frame(marks::COMMENTS_LOADED);
                    }
                    Err(e) => w.comments_state = Comments::Failed(friendly(&e)),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::ChannelTab(tab) => {
                let Some(Body::Channel(c)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                if c.tab == tab && c.load == Load::Ready {
                    return Task::none();
                }
                c.tab = tab;
                c.sort = None;
                c.sort_pending = None;
                self.load_top()
            }
            Message::ChannelSort(index) => {
                let page = self.top_id();
                let Some(Body::Channel(c)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                let Some(chip) = c.info.as_ref().and_then(|i| i.video_sort.get(index)) else {
                    return Task::none();
                };
                let Some(cursor) = &chip.next else {
                    if chip.selected && c.sort != Some(index) {
                        c.sort_pending = None;
                        return self.load_top();
                    }
                    return Task::none();
                };
                c.load = Load::Loading;
                c.sort_pending = Some(index);
                c.req += 1;
                let req = c.req;
                Task::perform(client().more_videos(cursor), move |r| {
                    Message::ChannelSorted(page, req, index, r)
                })
            }
            Message::ChannelSorted(page, req, index, result) => {
                let Some(Page {
                    body: Body::Channel(c),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != c.req {
                    return Task::none();
                }
                match result {
                    Ok(videos) => {
                        c.feed.set(videos);
                        c.sort = Some(index);
                        c.sort_pending = None;
                        c.load = Load::Ready;
                        scroll.reset();
                    }
                    Err(e) => c.load = Load::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                Task::none()
            }
            Message::ChannelAbout(open) => {
                let page = self.top_id();
                let Some(Body::Channel(c)) = self.top_mut().map(|p| &mut p.body) else {
                    return Task::none();
                };
                c.desc_open = open;
                if !open || c.about.is_some() || c.about_load == Load::Loading {
                    return Task::none();
                }
                let Some(cursor) = c.info.as_ref().and_then(|i| i.about.as_ref()) else {
                    return Task::none();
                };
                c.about_load = Load::Loading;
                Task::perform(client().channel_about(cursor), move |r| {
                    Message::ChannelAboutLoaded(page, r)
                })
            }
            Message::ChannelAboutLoaded(page, result) => {
                let Some(Body::Channel(c)) = self.page_mut(page).map(|p| &mut p.body) else {
                    return Task::none();
                };
                match result {
                    Ok(about) => {
                        c.about = Some(about);
                        c.about_load = Load::Ready;
                    }
                    Err(e) => c.about_load = Load::Failed(friendly(&e)),
                }
                Task::none()
            }
            Message::ChannelPlaylistsLoaded(page, req, result) => {
                let Some(Page {
                    body: Body::Channel(c),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != c.req {
                    return Task::none();
                }
                let Some(info) = &mut c.info else {
                    return Task::none();
                };
                match result {
                    Ok(page) => {
                        let mut ids: HashSet<_> =
                            info.playlists.items.iter().map(|p| p.id.clone()).collect();
                        info.playlists
                            .items
                            .extend(page.items.into_iter().filter(|p| ids.insert(p.id.clone())));
                        info.playlists.next = page.next;
                        c.playlists_more = More::Idle;
                    }
                    Err(e) => c.playlists_more = More::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
                Task::none()
            }
            Message::ChannelLoaded(page, req, r) => {
                let Some(Page {
                    body: Body::Channel(c),
                    scroll,
                    ..
                }) = self.page_mut(page)
                else {
                    return Task::none();
                };
                if req != c.req {
                    return Task::none();
                }
                match *r {
                    Ok(mut info) => {
                        if let Some(featured) = info.featured.take() {
                            info.videos.items.insert(0, featured);
                        }
                        c.feed.set(std::mem::take(&mut info.videos));
                        c.sort = info.video_sort.iter().position(|chip| chip.selected);
                        c.playlists_more = More::Idle;
                        c.info = Some(info);
                        c.load = Load::Ready;
                        scroll.reset();
                        mark_on_next_frame(marks::CHANNEL_LOADED);
                    }
                    Err(e) => c.load = Load::Failed(friendly(&e)),
                }
                scroll.rearm_near_end();
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
            Message::Retry => {
                if let Some(Body::Channel(c)) = self.top().map(|p| &p.body)
                    && let Some(index) = c.sort_pending
                {
                    return self.update(Message::ChannelSort(index));
                }
                self.load_top()
            }
            Message::RetryMore => {
                let Some(page) = self.top_mut() else {
                    return Task::none();
                };
                page.scroll.rearm_near_end();
                if let Body::Channel(c) = &mut page.body
                    && c.tab == nt_core::ChannelTab::Playlists
                {
                    return Self::load_channel_playlists(page.id, c);
                }
                match &mut page.body {
                    Body::Search(Search { feed, moments, .. }) => {
                        for f in [feed, moments] {
                            if matches!(f.more, Some(More::Failed(_))) {
                                f.more = Some(More::Idle);
                            }
                        }
                    }
                    Body::Home(Home { feed, .. }) | Body::Channel(Channel { feed, .. }) => {
                        feed.more = Some(More::Idle);
                    }
                    _ => {}
                }
                self.maybe_more()
            }
            Message::RetryComments => {
                let sort = self.watch().and_then(|w| w.comments_pending_sort);
                if let Some(index) = sort {
                    return self.update(Message::SortComments(index));
                }
                let id = self.top_id();
                match self.top_mut().map(|p| &mut p.body) {
                    Some(Body::Watch(w)) => Self::load_comments(id, w),
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
                    self.close_suggestions();
                    unfocus()
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
                    scroll,
                    ..
                }) = self.top_mut()
                {
                    if !w.comments_open.remove(&i) {
                        w.comments_open.insert(i);
                    }
                    scroll.forget_height(crate::view::comment_key(i));
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
                self.stop_preview();
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
            Message::ContextClose => {
                self.card_menu = None;
                self.player_context = None;
                Task::none()
            }
            Message::CardPlay => match self.menu_video().cloned() {
                Some(video) => self.open_video(video),
                None => Task::none(),
            },
            Message::PlayerContext => {
                self.menu = None;
                self.player_context = Some(last_cursor());
                Task::none()
            }
            Message::CopyVideoUrl(with_time) => {
                self.player_context = None;
                let Some(w) = self.watch() else {
                    return Task::none();
                };
                let mut url = watch_url(&w.video.id);
                if with_time {
                    url.push_str(&format!("&t={}s", self.play.position.max(0.0) as u64));
                }
                Task::batch([
                    iced::clipboard::write(url),
                    self.toast("Link copied to clipboard".into()),
                ])
            }
            Message::Stats(on) => {
                self.player_context = None;
                self.stats = on;
                Task::none()
            }
            Message::Guide => {
                if self.watch().is_some() || self.win.width < 792.0 {
                    self.guide_overlay = !self.guide_overlay;
                } else {
                    self.prefs.guide = !self.prefs.guide;
                    self.prefs.save();
                }
                Task::none()
            }
            Message::Shortcuts(on) => {
                self.close_suggestions();
                self.shortcuts = on;
                Task::none()
            }
            Message::Tab(reverse) => {
                if reverse {
                    iced::widget::operation::focus_previous()
                } else {
                    iced::widget::operation::focus_next()
                }
            }
            Message::Sleep(timer) => {
                self.sleep = timer;
                if timer == SleepTimer::EndOfVideo {
                    crate::scroller::trace_event("sleep_timer_end_of_video_armed");
                }
                self.prepare_successor();
                self.sleep_seq += 1;
                self.menu = Some(MenuPage::Main);
                if let SleepTimer::Minutes(minutes) = timer {
                    let seq = self.sleep_seq;
                    Task::future(async move {
                        tokio::time::sleep(Duration::from_secs(u64::from(minutes) * 60)).await;
                        Message::SleepFired(seq)
                    })
                } else {
                    Task::none()
                }
            }
            Message::SleepFired(seq) if seq == self.sleep_seq => self.fire_sleep(),
            Message::SleepFired(_) => Task::none(),
            Message::CardSaveLater => {
                let Some(v) = self.menu_video().cloned() else {
                    return Task::none();
                };
                let now = library().toggle_watch_later(&v);
                if !now && let Some(Body::Later(items)) = self.top_mut().map(|p| &mut p.body) {
                    items.retain(|item| item.id != v.id);
                }
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
            Message::Queue(open) => {
                self.close_suggestions();
                self.queue_open = open;
                Task::none()
            }
            Message::QueueAdd => {
                let Some(video) = self.menu_video().cloned() else {
                    return Task::none();
                };
                library().enqueue(&video);
                self.update_queue();
                self.toast("Added to queue".into())
            }
            Message::QueueRemove(id) => {
                library().remove_queued(&id);
                self.update_queue();
                Task::none()
            }
            Message::QueuePlay(id) => {
                let Some(v) = self.queue.iter().find(|v| v.id == id).cloned() else {
                    return Task::none();
                };
                library().remove_queued(&id);
                self.queue = library().queue();
                self.open_video(v)
            }
            Message::CardNotInterested | Message::CardHideChannel => {
                let Some(video) = self.menu_video().cloned() else {
                    return Task::none();
                };
                if matches!(message, Message::CardHideChannel) {
                    let Some(id) = &video.channel_id else {
                        return Task::none();
                    };
                    library().set_channel_hidden(id, true);
                } else {
                    library().set_video_hidden(&video.id, true);
                }
                for page in self.pages.iter_mut().chain(&mut self.forward) {
                    match &mut page.body {
                        Body::Home(h) => h.feed.items.retain(|v| !library().is_hidden(v)),
                        Body::Search(s) => {
                            let mut kept = HashMap::new();
                            let mut next = 0;
                            for (i, v) in s.feed.items.iter().enumerate() {
                                if !library().is_hidden(v) {
                                    kept.insert(i, next);
                                    next += 1;
                                }
                            }
                            s.feed.items.retain(|v| !library().is_hidden(v));
                            s.moments.items.retain(|v| !library().is_hidden(v));
                            s.entries.retain_mut(|entry| {
                                let SearchEntry::Video(index) = entry else {
                                    return true;
                                };
                                let Some(i) = kept.get(index) else {
                                    return false;
                                };
                                *index = *i;
                                true
                            });
                        }
                        Body::Watch(w) => {
                            w.related.retain(|v| !library().is_hidden(v));
                            if let Some(initial) = &mut w.initial_related {
                                initial.items.retain(|v| !library().is_hidden(v));
                            }
                        }
                        Body::Subs(s) => s.feed.items.retain(|v| !library().is_hidden(v)),
                        Body::Channel(c) => c.feed.items.retain(|v| {
                            v.id != video.id || matches!(message, Message::CardHideChannel)
                        }),
                        _ => {}
                    }
                    page.scroll.rearm_near_end();
                }
                self.prepare_successor();
                self.toast("Recommendation removed".into())
            }
            Message::Toast(t) => self.toast(t),
            Message::ToastDone(seq) => {
                if self.toast.as_ref().is_some_and(|(_, s)| *s == seq) {
                    self.toast = None;
                }
                Task::none()
            }
            Message::ToggleCaptions => self.toggle_captions(),
            Message::SetCaptions(selection) => {
                if self.menu == Some(MenuPage::Captions) {
                    self.menu = Some(MenuPage::Main);
                } else if self.menu == Some(MenuPage::Translate) {
                    self.menu = Some(MenuPage::Captions);
                }
                self.set_captions(selection)
            }
            Message::PlayerSettingDone(page, r) => {
                if self.playing_page == Some(page)
                    && let Err(e) = r
                {
                    if let Some(p) = ext::existing_player() {
                        self.play = normalized_playback(p.state());
                    }
                    return self.toast(friendly(&e));
                }
                Task::none()
            }
            Message::SetAudio(id) => {
                let page = self.top_id();
                let Some(p) = ext::existing_player().filter(|_| self.playing_page == Some(page))
                else {
                    return Task::none();
                };
                self.menu = Some(MenuPage::Main);
                Task::perform(p.set_audio_track(&id), move |r| {
                    Message::PlayerSettingDone(page, r)
                })
            }
            Message::Normalize => {
                self.player_do(|p| p.set_normalize(!self.play.normalize));
                Task::none()
            }
            Message::Loop => {
                self.player_context = None;
                self.play.looping = !self.play.looping;
                let looping = self.play.looping;
                self.player_do(|p| p.set_loop(looping));
                self.prepare_successor();
                Task::none()
            }
            Message::ScrubHover(seconds) => self.scrub_hover(seconds),
            Message::TestCycle => self.test_cycle_step(),
            Message::ControlsShown(shown) => {
                let to = if shown { 1.0 } else { 0.0 };
                if self.caption_anim.map_or(self.caption_lift, |(_, t, _)| t) != to {
                    self.caption_anim = Some((self.caption_lift, to, Instant::now()));
                }
                Task::none()
            }
            Message::CaptionFrame(now) => {
                if let Some((from, to, start)) = self.caption_anim {
                    // 200 ms, ease out, like YouTube's caption move.
                    let t = (now.saturating_duration_since(start).as_secs_f32() / 0.2).min(1.0);
                    let eased = 1.0 - (1.0 - t).powi(3);
                    self.caption_lift = from + (to - from) * eased;
                    if t >= 1.0 {
                        self.caption_anim = None;
                    }
                    self.place_captions();
                }
                Task::none()
            }
            Message::ScrubImage(req, result) => {
                if req == self.scrub_request
                    && let Ok(frame) = result
                {
                    self.scrub_preview = Some(ImageHandle::from_rgba(
                        frame.width,
                        frame.height,
                        frame.pixels.clone(),
                    ));
                }
                Task::none()
            }
            Message::ShowChapters => {
                if let Some(Page {
                    body: Body::Watch(w),
                    scroll,
                    ..
                }) = self.top_mut()
                {
                    w.desc_open = true;
                    let y = w.desc_y.get();
                    if y.is_finite() {
                        scroll.with(|a, now| a.scroll_to(y as f64, now));
                    }
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
                self.prepare_successor();
                if self.menu == Some(MenuPage::Quality) {
                    self.menu = Some(MenuPage::Main);
                }
                Task::none()
            }
            Message::ToggleAutoplay => {
                self.prefs.autoplay = !self.prefs.autoplay;
                self.prepare_successor();
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
                self.sync_player_geometry();
                self.prefs.save();
                Task::none()
            }
            Message::Escape => {
                let closed_popup = self.card_menu.take().is_some()
                    || self.player_context.take().is_some()
                    || self.menu.take().is_some();
                if self.queue_open {
                    self.queue_open = false;
                } else if self.search_filters {
                    self.search_filters = false;
                } else if self.shortcuts {
                    self.shortcuts = false;
                } else if closed_popup {
                } else if self.guide_overlay {
                    self.guide_overlay = false;
                } else if self.fullscreen {
                    return self.set_fullscreen(false);
                }
                Task::none()
            }
            Message::ResumeLast => {
                let Some(last) = self.resume_offer.take() else {
                    return Task::none();
                };
                let video = library()
                    .history()
                    .into_iter()
                    .find(|v| v.id == last.id)
                    .unwrap_or_else(|| Video {
                        thumbnail: Video::thumbnail_url(&last.id),
                        id: last.id,
                        title: last.title,
                        ..Video::default()
                    });
                self.open_video_at(video, last.position)
            }
            Message::RetryPlayback => {
                let Some(top) = self.top() else {
                    return Task::none();
                };
                if let Body::Watch(w) = &top.body {
                    let (id, vid, at) =
                        (top.id, w.video.id.clone(), self.play.position.max(w.resume));
                    self.parked = None;
                    self.start_playback(id, &vid, at, false);
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
                    crate::scroller::trace_event("autoplay_advance");
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
                self.up_next = None;
                if let Some(video) = library().pop_queued() {
                    // Keep the prepared head available until load_at has consumed it.
                    self.queue = library().queue();
                    return self.open_video(video);
                }
                if let Some(index) = self
                    .watch()
                    .and_then(|w| w.playlist.as_ref())
                    .filter(|list| list.next_video().is_some())
                    .and_then(|list| list.index)
                    .and_then(|i| i.checked_add(1))
                {
                    return self.play_playlist_row(index);
                }
                match self.watch().and_then(|w| w.related.first()).cloned() {
                    Some(video) => self.open_video(video),
                    None => Task::none(),
                }
            }
            Message::WindowSize(s) => {
                self.win = Size::new(s.width.max(1.0), s.height.max(1.0));
                self.sync_player_geometry();
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
                    self.sync_player_geometry();
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
            keyboard::Key::Character("c") => Message::ToggleCaptions,
            keyboard::Key::Character("N" | "n") if mods.shift() => Message::UpNextPlay,
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
                    chips: Vec::new(),
                    show_initial: idx == 0,
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
                } else if self.list(ListRef::Main).is_empty() {
                    // A search answered only by transcript matches (e.g. "pronsh").
                    ListRef::Moments
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
            match self.caption_anim {
                Some(_) => window::frames().map(Message::CaptionFrame),
                None => Subscription::none(),
            },
            match test_cycle() {
                Some(_) => {
                    iced::time::every(Duration::from_millis(400)).map(|_| Message::TestCycle)
                }
                None => Subscription::none(),
            },
        ])
    }

    /// `NT_TEST_CYCLE=theatre|fullscreen|stats|scrub|menus` toggles that part of the watch
    /// page every 400 ms, for memory and stability runs without real input.
    fn test_cycle_step(&mut self) -> Task<Message> {
        if self.watch().is_none() || self.playing_page.is_none() {
            return Task::none();
        }
        self.test_step = self.test_step.wrapping_add(1);
        let step = self.test_step;
        match test_cycle() {
            Some("theatre") => self.update(Message::ToggleTheatre),
            Some("fullscreen") => self.update(Message::ToggleFullscreen),
            Some("stats") => self.update(Message::Stats(!self.stats)),
            Some("scrub") => {
                // Hover previews across the bar, and a seek every fourth step.
                let frac = ((step * 37) % 100) as f64 / 100.0;
                let at = frac * self.play.duration.max(1.0);
                let hover = self.update(Message::ScrubHover(Some(at)));
                if step.is_multiple_of(4) {
                    Task::batch([hover, self.update(Message::SeekFrac(frac as f32))])
                } else {
                    hover
                }
            }
            Some("controls") => {
                // Controls shown, then hidden, about 3.2 s each.
                if step.is_multiple_of(8) {
                    self.test_controls = !self.test_controls;
                    crate::scroller::trace_event(if self.test_controls {
                        "test_controls_shown"
                    } else {
                        "test_controls_hidden"
                    });
                }
                Task::none()
            }
            Some("menus") => match step % 4 {
                0 => self.update(Message::Menu(Some(MenuPage::Main))),
                1 => self.update(Message::Menu(Some(MenuPage::Quality))),
                2 => self.update(Message::PlayerContext),
                _ => self.update(Message::Escape),
            },
            _ => Task::none(),
        }
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
        keyboard::Key::Named(Named::Tab) if status == event::Status::Ignored => {
            Some(Message::Tab(mods.shift()))
        }
        keyboard::Key::Character("?") if status == event::Status::Ignored => {
            Some(Message::Shortcuts(true))
        }
        keyboard::Key::Named(Named::Escape) if status == event::Status::Ignored => {
            Some(Message::Escape)
        }
        _ => None,
    }
}

/// Test-only: `NT_SLEEP_TIMER=end` arms the "End of video" sleep timer at start, so an
/// autopilot run can check it without opening the settings menu.
fn test_sleep_timer() -> SleepTimer {
    if std::env::var("NT_SLEEP_TIMER").as_deref() == Ok("end") {
        crate::scroller::trace_event("sleep_timer_end_of_video_armed");
        SleepTimer::EndOfVideo
    } else {
        SleepTimer::Off
    }
}

/// A test-only switch from the environment (`NAME=1`).
fn test_flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

fn test_cycle() -> Option<&'static str> {
    static CYCLE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CYCLE
        .get_or_init(|| std::env::var("NT_TEST_CYCLE").ok())
        .as_deref()
}

/// Clear keyboard focus (the search field after a search). iced 0.14 has this operation in
/// `advanced`, though `widget::operation` only re-exports `focus`.
fn unfocus() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
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

fn normalized_playback(mut state: PlaybackState) -> PlaybackState {
    if !state.speed.is_finite() || state.speed <= 0.0 {
        state.speed = 1.0;
    }
    state
}
