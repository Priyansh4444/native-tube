//! Application state, messages and `update`.
//!
//! State is a navigation stack of pages; each page owns its data and its `ScrollModel`, so a
//! page under the top keeps everything and Back restores it as it was. Network work is a
//! `nt_core::Job` turned into an iced `Task`; results come back as messages tagged with the
//! page id (and a request number where one page can ask twice), so stale answers are dropped.

use crate::ext::{self, Ext};
use crate::imgcache::Images;
use crate::scroller::{ScrollModel, mark_on_next_frame};
use iced::keyboard::{self, key::Named};
use iced::{Size, Subscription, Task, event, mouse, window};
use nt_core::autopilot::Command;
use nt_core::metrics::marks;
use nt_core::{
    ChannelInfo, ChannelRef, Comment, CommentPage, Continuation, PixelFormat, PlaybackState, Player, Status, TOPICS, Video,
    VideoFrame, WatchInfo, client, library,
};
use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

/// The search field's widget id (for the `/` shortcut).
pub const SEARCH_ID: &str = "search";

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

#[derive(Debug, Clone)]
pub enum Message {
    Nav(Root),
    Back,
    Pilot(Command),
    Ext(Ext),
    ScrollMoved,
    Topic(usize),
    Open(ListRef, usize),
    OpenChannelOf(ListRef, usize),
    OpenChannel(ChannelRef),
    OpenWatchChannel,
    FeedLoaded { page: u64, req: u64, append: bool, result: nt_core::Result<nt_core::Page<Video>> },
    WatchLoaded(u64, nt_core::Result<WatchInfo>),
    CommentsLoaded(u64, nt_core::Result<CommentPage>),
    ChannelLoaded(u64, nt_core::Result<ChannelInfo>),
    SubsLoaded(u64, nt_core::Result<Vec<Video>>),
    Retry,
    RetryMore,
    LoadComments,
    SearchInput(String),
    SearchSubmit,
    SearchFocus(bool),
    SuggestNav(i32),
    SuggestClose,
    SuggestFire(u64),
    Suggestions(u64, nt_core::Result<Vec<String>>),
    SuggestPick(usize),
    ToggleSubscribe,
    ToggleLater,
    ClearHistory,
    ToggleDesc,
    PlayPause,
    SeekFrac(f32),
    SeekBy(f64),
    SetVolume(f32),
    VolumeBy(f64),
    ToggleMute,
    RetryPlayback,
    WindowSize(Size),
    Scale(f32),
    FocusSearch,
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
    pub next: Option<Continuation>,
    pub more: Option<More>,
}

impl Feed {
    fn set(&mut self, page: nt_core::Page<Video>) {
        self.items = page.items;
        self.next = page.next;
        self.more = Some(More::Idle);
    }
    fn append(&mut self, page: nt_core::Page<Video>) {
        for v in page.items {
            if !self.items.iter().any(|x| x.id == v.id) {
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

pub struct Watch {
    pub video: Video,
    pub load: Load,
    pub info: Option<WatchInfo>,
    pub related: Vec<Video>,
    pub comments: Vec<Comment>,
    pub comments_next: Option<Continuation>,
    pub comments_total: Option<String>,
    pub comments_state: Comments,
    pub desc_open: bool,
    pub resume: f64,
    /// Where the comments header sits in the content, as of the last build.
    pub comments_y: Cell<f32>,
}

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

pub struct App {
    pub pages: Vec<Page>,
    next_id: u64,
    pub win: Size,
    pub scale: f32,
    pub images: Images,
    // Search field and suggestions.
    pub query: String,
    pub suggestions: Vec<String>,
    pub sugg_sel: Option<usize>,
    pub sugg_open: bool,
    sugg_seq: u64,
    pub search_focused: bool,
    pub blur: u64,
    // Playback.
    pub player: Option<Player>,
    pub play: PlaybackState,
    pub frame: Option<Arc<VideoFrame>>,
    frame_gen: u64,
    playing_page: Option<u64>,
    resume_pending: Option<f64>,
    want_playback_mark: bool,
    pub player_error: Option<String>,
}

impl App {
    pub fn boot() -> (App, Task<Message>) {
        let mut app = App {
            pages: Vec::new(),
            next_id: 1,
            win: Size::new(1280.0, 800.0),
            scale: 1.0,
            images: Images::default(),
            query: String::new(),
            suggestions: Vec::new(),
            sugg_sel: None,
            sugg_open: false,
            sugg_seq: 0,
            search_focused: false,
            blur: 0,
            player: None,
            play: PlaybackState::default(),
            frame: None,
            frame_gen: 0,
            playing_page: None,
            resume_pending: None,
            want_playback_mark: false,
            player_error: None,
        };
        let task = app.replace_root(Root::Home);
        mark_on_next_frame(marks::FIRST_FRAME);
        let scale = window::latest().and_then(window::scale_factor).map(Message::Scale);
        (app, Task::batch([task, scale]))
    }

    pub fn top(&self) -> &Page {
        &self.pages[self.pages.len() - 1]
    }
    fn top_mut(&mut self) -> &mut Page {
        let n = self.pages.len();
        &mut self.pages[n - 1]
    }

    pub fn root(&self) -> Option<Root> {
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
        Page { id, scroll: ScrollModel::default(), body }
    }

    // ---- navigation ---------------------------------------------------------------------

    fn leave_top(&mut self) {
        let Some(page) = self.pages.last_mut() else { return };
        if let Body::Watch(w) = &mut page.body
            && self.playing_page == Some(page.id)
        {
            w.resume = self.play.position;
            if let Some(p) = &self.player {
                p.stop();
            }
            self.playing_page = None;
            self.frame = None;
            self.resume_pending = None;
            self.want_playback_mark = false;
        }
    }

    fn push(&mut self, body: Body) -> u64 {
        self.leave_top();
        self.close_suggestions();
        let page = self.new_page(body);
        let id = page.id;
        self.pages.push(page);
        id
    }

    fn replace_root(&mut self, root: Root) -> Task<Message> {
        self.leave_top();
        self.close_suggestions();
        let body = match root {
            Root::Home => Body::Home(Home { topic: 0, load: Load::Loading, feed: Feed::default(), req: 0 }),
            Root::Subscriptions => {
                let channels = library().subscriptions();
                let load = if channels.is_empty() { Load::Ready } else { Load::Loading };
                Body::Subs(Subs { channels, load, feed: Feed::default() })
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
        self.pages.pop();
        let top = self.top();
        match &top.body {
            Body::Watch(w) if w.load == Load::Ready || w.info.is_none() => {
                let (id, vid, resume) = (top.id, w.video.id.clone(), w.resume);
                self.start_playback(id, &vid, resume);
            }
            Body::History(_) => {
                let items = library().history();
                if let Body::History(v) = &mut self.top_mut().body {
                    *v = items;
                }
            }
            Body::Later(_) => {
                let items = library().watch_later();
                if let Body::Later(v) = &mut self.top_mut().body {
                    *v = items;
                }
            }
            _ => {}
        }
        Task::none()
    }

    /// Start (or restart after an error) whatever the top page needs from the network.
    fn load_top(&mut self) -> Task<Message> {
        let page = self.top_mut();
        let id = page.id;
        page.scroll.rearm_near_end();
        match &mut page.body {
            Body::Home(h) => {
                h.load = Load::Loading;
                h.req += 1;
                let req = h.req;
                Task::perform(client().home(TOPICS[h.topic].id), move |result| Message::FeedLoaded {
                    page: id,
                    req,
                    append: false,
                    result,
                })
            }
            Body::Search(s) => {
                s.load = Load::Loading;
                Task::perform(client().search(&s.query), move |result| Message::FeedLoaded {
                    page: id,
                    req: 0,
                    append: false,
                    result,
                })
            }
            Body::Watch(w) => {
                w.load = Load::Loading;
                Task::perform(client().watch(&w.video.id), move |r| Message::WatchLoaded(id, r))
            }
            Body::Channel(c) => {
                c.load = Load::Loading;
                Task::perform(client().channel(&c.id), move |r| Message::ChannelLoaded(id, r))
            }
            Body::Subs(s) if !s.channels.is_empty() => {
                s.load = Load::Loading;
                Task::perform(client().subscription_feed(s.channels.clone()), move |r| Message::SubsLoaded(id, r))
            }
            _ => Task::none(),
        }
    }

    fn open_video(&mut self, video: Video) -> Task<Message> {
        library().record_watch(&video);
        let vid = video.id.clone();
        let watch = Watch {
            video,
            load: Load::Loading,
            info: None,
            related: Vec::new(),
            comments: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: Comments::NotStarted,
            desc_open: false,
            resume: 0.0,
            comments_y: Cell::new(f32::INFINITY),
        };
        let id = self.push(Body::Watch(Box::new(watch)));
        self.start_playback(id, &vid, 0.0);
        self.load_top()
    }

    fn open_channel(&mut self, stub: ChannelRef) -> Task<Message> {
        if stub.id.is_empty() {
            return Task::none();
        }
        let id = stub.id.clone();
        self.push(Body::Channel(Channel { id, stub, load: Load::Loading, info: None, feed: Feed::default() }));
        self.load_top()
    }

    fn submit_search(&mut self, query: String) -> Task<Message> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Task::none();
        }
        self.query = query.clone();
        self.blur += 1;
        self.push(Body::Search(Search { query, load: Load::Loading, feed: Feed::default() }));
        self.load_top()
    }

    fn list(&self, which: ListRef) -> &[Video] {
        match (&self.top().body, which) {
            (Body::Home(h), ListRef::Main) => &h.feed.items,
            (Body::Search(s), ListRef::Main) => &s.feed.items,
            (Body::Channel(c), ListRef::Main) => &c.feed.items,
            (Body::Subs(s), ListRef::Main) => &s.feed.items,
            (Body::History(v), ListRef::Main) | (Body::Later(v), ListRef::Main) => v,
            (Body::Watch(w), _) => &w.related,
            _ => &[],
        }
    }

    // ---- playback -------------------------------------------------------------------------

    fn start_playback(&mut self, page: u64, video_id: &str, resume: f64) {
        if self.player.is_none() {
            match Player::new(PixelFormat::Rgba, ext::player_waker()) {
                Ok(p) => self.player = Some(p),
                Err(e) => {
                    self.player_error = Some(e.to_string());
                    return;
                }
            }
        }
        self.player_error = None;
        self.frame = None;
        if let Some(p) = &self.player {
            match p.load(video_id) {
                Ok(()) => {
                    self.playing_page = Some(page);
                    self.resume_pending = (resume > 1.0).then_some(resume);
                    self.want_playback_mark = true;
                    self.play = p.state();
                }
                Err(e) => self.player_error = Some(e.to_string()),
            }
        }
    }

    fn on_player(&mut self) {
        ext::player_taken();
        let Some(p) = &self.player else { return };
        self.play = p.state();
        if self.playing_page.is_none() {
            self.frame = None;
            return;
        }
        let g = p.frame_generation();
        if g != self.frame_gen {
            self.frame_gen = g;
            self.frame = p.frame();
            if self.frame.is_some() && self.want_playback_mark {
                self.want_playback_mark = false;
                mark_on_next_frame(marks::PLAYBACK_STARTED);
            }
        }
        if let Some(t) = self.resume_pending
            && self.play.status == Status::Ready
        {
            p.seek(t);
            self.resume_pending = None;
        }
    }

    // ---- infinite scroll ------------------------------------------------------------------

    fn maybe_more(&mut self) -> Task<Message> {
        let page = self.top_mut();
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
                return if go { Self::load_comments(id, w) } else { Task::none() };
            }
            _ => None,
        };
        let Some((feed, req)) = feed else { return Task::none() };
        if !near || feed.more != Some(More::Idle) {
            return Task::none();
        }
        let Some(next) = feed.next.clone() else { return Task::none() };
        feed.more = Some(More::Loading);
        Task::perform(client().more_videos(&next), move |result| Message::FeedLoaded { page: id, req, append: true, result })
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
        let Some(next) = next else { return Task::none() };
        w.comments_state = Comments::Loading;
        Task::perform(client().more_comments(&next), move |r| Message::CommentsLoaded(id, r))
    }

    fn close_suggestions(&mut self) {
        self.sugg_open = false;
        self.sugg_sel = None;
        self.sugg_seq += 1;
    }

    // ---- update ---------------------------------------------------------------------------

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Nav(root) => self.replace_root(root),
            Message::Back => self.back(),
            Message::Pilot(cmd) => self.pilot(cmd),
            Message::Ext(Ext::Image(url, w, r)) => {
                self.images.loaded(url, w, r);
                Task::none()
            }
            Message::Ext(Ext::Player) => {
                self.on_player();
                Task::none()
            }
            Message::ScrollMoved => self.maybe_more(),
            Message::Topic(i) => {
                if let Body::Home(h) = &mut self.top_mut().body {
                    if h.topic == i && h.load != Load::Loading {
                        return Task::none();
                    }
                    h.topic = i;
                    h.feed = Feed::default();
                }
                self.top().scroll.with(|a, _| a.jump_to(0.0));
                self.load_top()
            }
            Message::Open(list, i) => match self.list(list).get(i).cloned() {
                Some(v) => self.open_video(v),
                None => Task::none(),
            },
            Message::OpenChannelOf(list, i) => {
                let Some(v) = self.list(list).get(i) else { return Task::none() };
                let Some(id) = v.channel_id.clone() else { return Task::none() };
                let stub = ChannelRef { id, name: v.channel.clone(), avatar: v.channel_avatar.clone(), subscribers: None };
                self.open_channel(stub)
            }
            Message::OpenChannel(c) => self.open_channel(c),
            Message::OpenWatchChannel => {
                let stub = match &self.top().body {
                    Body::Watch(w) => match &w.info {
                        Some(i) if !i.channel.id.is_empty() => Some(i.channel.clone()),
                        _ => w.video.channel_id.clone().map(|id| ChannelRef {
                            id,
                            name: w.video.channel.clone(),
                            avatar: w.video.channel_avatar.clone(),
                            subscribers: None,
                        }),
                    },
                    _ => None,
                };
                stub.map(|s| self.open_channel(s)).unwrap_or_else(Task::none)
            }
            Message::FeedLoaded { page, req, append, result } => {
                let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return Task::none() };
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
                    }
                    (Ok(pg), true) => {
                        feed.append(pg);
                        mark_on_next_frame(marks::MORE_LOADED);
                    }
                    (Err(e), false) => *load = Load::Failed(e.to_string()),
                    (Err(e), true) => feed.more = Some(More::Failed(e.to_string())),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::WatchLoaded(page, r) => {
                let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return Task::none() };
                let Body::Watch(w) = &mut p.body else { return Task::none() };
                match r {
                    Ok(mut info) => {
                        // Core workaround (NOTES.md): some live watch responses carry no
                        // channel; the card that opened the video knows it.
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
                        w.related = info.related.items.clone();
                        w.info = Some(info);
                        w.load = Load::Ready;
                        mark_on_next_frame(marks::WATCH_LOADED);
                    }
                    Err(e) => w.load = Load::Failed(e.to_string()),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::CommentsLoaded(page, r) => {
                let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return Task::none() };
                let Body::Watch(w) = &mut p.body else { return Task::none() };
                match r {
                    Ok(cp) => {
                        if cp.total.is_some() {
                            w.comments_total = cp.total;
                        }
                        w.comments.extend(cp.items);
                        w.comments_next = cp.next;
                        w.comments_state = Comments::Idle;
                        mark_on_next_frame(marks::COMMENTS_LOADED);
                    }
                    Err(e) => w.comments_state = Comments::Failed(e.to_string()),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::ChannelLoaded(page, r) => {
                let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return Task::none() };
                let Body::Channel(c) = &mut p.body else { return Task::none() };
                match r {
                    Ok(mut info) => {
                        c.feed.set(std::mem::take(&mut info.videos));
                        c.info = Some(info);
                        c.load = Load::Ready;
                        mark_on_next_frame(marks::CHANNEL_LOADED);
                    }
                    Err(e) => c.load = Load::Failed(e.to_string()),
                }
                p.scroll.rearm_near_end();
                Task::none()
            }
            Message::SubsLoaded(page, r) => {
                let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return Task::none() };
                let Body::Subs(s) = &mut p.body else { return Task::none() };
                match r {
                    Ok(items) => {
                        s.feed.items = items;
                        s.load = Load::Ready;
                    }
                    Err(e) => s.load = Load::Failed(e.to_string()),
                }
                Task::none()
            }
            Message::Retry => {
                if let Body::Watch(w) = &mut self.top_mut().body
                    && let Comments::Failed(_) = w.comments_state
                    && w.load == Load::Ready
                {
                    w.comments_state = Comments::Idle;
                    let id = self.top().id;
                    let Body::Watch(w) = &mut self.top_mut().body else { return Task::none() };
                    return Self::load_comments(id, w);
                }
                self.load_top()
            }
            Message::RetryMore => {
                let page = self.top_mut();
                page.scroll.rearm_near_end();
                match &mut page.body {
                    Body::Home(Home { feed, .. }) | Body::Search(Search { feed, .. }) | Body::Channel(Channel { feed, .. }) => {
                        feed.more = Some(More::Idle);
                    }
                    _ => {}
                }
                self.maybe_more()
            }
            Message::LoadComments => {
                let id = self.top().id;
                match &mut self.top_mut().body {
                    Body::Watch(w) => {
                        if w.comments_state == Comments::Loading {
                            Task::none()
                        } else {
                            Self::load_comments(id, w)
                        }
                    }
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
                Task::perform(client().suggestions(&self.query), move |r| Message::Suggestions(seq, r))
            }
            Message::SuggestFire(_) => Task::none(),
            Message::Suggestions(seq, r) => {
                if seq == self.sugg_seq
                    && let Ok(mut list) = r
                {
                    list.truncate(10);
                    self.sugg_open = !list.is_empty() && self.search_focused;
                    self.suggestions = list;
                    self.sugg_sel = None;
                }
                Task::none()
            }
            Message::SuggestNav(d) => {
                if !self.sugg_open || self.suggestions.is_empty() {
                    return Task::none();
                }
                let n = self.suggestions.len() as i32;
                let cur = self.sugg_sel.map(|i| i as i32).unwrap_or(if d > 0 { -1 } else { n });
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
            Message::ToggleSubscribe => {
                let ch = match &self.top().body {
                    Body::Watch(w) => w.info.as_ref().map(|i| i.channel.clone()),
                    Body::Channel(c) => c.info.as_ref().map(|i| i.as_ref()).or(Some(c.stub.clone())),
                    _ => None,
                };
                if let Some(ch) = ch.filter(|c| !c.id.is_empty()) {
                    library().toggle_subscription(&ch);
                }
                Task::none()
            }
            Message::ToggleLater => {
                if let Body::Watch(w) = &self.top().body {
                    let mut v = w.info.as_ref().map(|i| i.as_video()).unwrap_or_else(|| w.video.clone());
                    if v.duration.is_none() {
                        v.duration = w.video.duration.clone();
                    }
                    v.is_live = w.video.is_live;
                    library().toggle_watch_later(&v);
                }
                Task::none()
            }
            Message::ClearHistory => {
                library().clear_history();
                if let Body::History(v) = &mut self.top_mut().body {
                    v.clear();
                }
                Task::none()
            }
            Message::ToggleDesc => {
                if let Body::Watch(w) = &mut self.top_mut().body {
                    w.desc_open = !w.desc_open;
                }
                Task::none()
            }
            Message::PlayPause => {
                if let Some(p) = &self.player {
                    p.toggle_pause();
                }
                Task::none()
            }
            Message::SeekFrac(f) => {
                if let Some(p) = &self.player
                    && self.play.duration > 0.0
                {
                    p.seek(f as f64 * self.play.duration);
                    self.play.position = f as f64 * self.play.duration;
                }
                Task::none()
            }
            Message::SeekBy(s) => {
                if let Some(p) = &self.player {
                    p.seek_by(s);
                }
                Task::none()
            }
            Message::SetVolume(v) => {
                if let Some(p) = &self.player {
                    p.set_volume(v as f64);
                    if self.play.muted {
                        p.set_muted(false);
                    }
                    self.play.volume = v as f64;
                }
                Task::none()
            }
            Message::VolumeBy(d) => {
                if let Some(p) = &self.player {
                    p.set_volume(self.play.volume + d);
                }
                Task::none()
            }
            Message::ToggleMute => {
                if let Some(p) = &self.player {
                    p.set_muted(!self.play.muted);
                }
                Task::none()
            }
            Message::RetryPlayback => {
                let top = self.top();
                if let Body::Watch(w) = &top.body {
                    let (id, vid, at) = (top.id, w.video.id.clone(), self.play.position.max(w.resume));
                    self.start_playback(id, &vid, at);
                }
                Task::none()
            }
            Message::FocusSearch => iced::widget::operation::focus(SEARCH_ID),
            Message::WindowSize(s) => {
                self.win = s;
                Task::none()
            }
            Message::Scale(s) => {
                self.scale = s;
                Task::none()
            }
        }
    }

    /// Keys the watch page takes before scrolling sees them.
    pub fn watch_key_message(key: &keyboard::Key, mods: keyboard::Modifiers) -> Option<Message> {
        if mods.control() || mods.alt() || mods.logo() {
            return None;
        }
        match key.as_ref() {
            keyboard::Key::Named(Named::Space) | keyboard::Key::Character("k") => Some(Message::PlayPause),
            keyboard::Key::Character("j") => Some(Message::SeekBy(-10.0)),
            keyboard::Key::Character("l") => Some(Message::SeekBy(10.0)),
            keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SeekBy(-5.0)),
            keyboard::Key::Named(Named::ArrowRight) => Some(Message::SeekBy(5.0)),
            keyboard::Key::Character("m") => Some(Message::ToggleMute),
            keyboard::Key::Named(Named::ArrowUp) => Some(Message::VolumeBy(5.0)),
            keyboard::Key::Named(Named::ArrowDown) => Some(Message::VolumeBy(-5.0)),
            _ => None,
        }
    }

    fn pilot(&mut self, cmd: Command) -> Task<Message> {
        let scroll = |app: &App, f: &dyn Fn(&mut nt_core::scroll::ScrollAnimator, f64)| {
            app.top().scroll.with(|a, now| f(a, now));
        };
        match cmd {
            Command::Home { topic } => {
                let idx = topic.and_then(|t| TOPICS.iter().position(|x| x.id == t)).unwrap_or(0);
                let t = self.replace_root(Root::Home);
                if idx != 0 {
                    return self.update(Message::Topic(idx));
                }
                t
            }
            Command::Search { query } => {
                self.query = query;
                self.update(Message::SearchSubmit)
            }
            Command::OpenVideo { index } => {
                let list = if matches!(self.top().body, Body::Watch(_)) { ListRef::Related } else { ListRef::Main };
                self.update(Message::Open(list, index))
            }
            Command::OpenChannel => self.update(Message::OpenWatchChannel),
            Command::Back => self.back(),
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
            Command::Quit => {
                if let Some(p) = &self.player {
                    p.stop();
                }
                iced::exit()
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(ext::events),
            Subscription::run(ext::autopilot),
            event::listen_with(|e, _status, _id| match e {
                iced::Event::Window(window::Event::Resized(s)) => Some(Message::WindowSize(s)),
                iced::Event::Window(window::Event::Opened { size, .. }) => Some(Message::WindowSize(size)),
                iced::Event::Window(window::Event::Rescaled(s)) => Some(Message::Scale(s)),
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)) => Some(Message::Back),
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Character(c), .. })
                    if c.as_str() == "/" && _status == iced::event::Status::Ignored =>
                {
                    Some(Message::FocusSearch)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(Named::ArrowLeft),
                    modifiers,
                    ..
                }) if modifiers.alt() => Some(Message::Back),
                _ => None,
            }),
        ])
    }

    pub fn is_subscribed(&self, id: &str) -> bool {
        !id.is_empty() && library().is_subscribed(id)
    }
}
