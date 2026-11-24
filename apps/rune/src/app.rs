//! App state, navigation, data loading, input routing and the per-frame `poll`.

use crate::images::Images;
use crate::pages::*;
use crate::scroller::{at, now};
use crate::video::{Menu, VideoState};
use nt_core::autopilot::{self, Command};
use nt_core::metrics::marks;
use nt_core::{Job, Video, client, library};
use rune::{AppHandle, GesturePhase, Key, KeyEvent, Live, TextEdit, Tx, WheelEvent};
use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

static HANDLE: OnceLock<AppHandle<App>> = OnceLock::new();
static PENDING_MARKS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

pub fn set_handle(h: AppHandle<App>) {
    let _ = HANDLE.set(h);
}

pub fn handle() -> Option<&'static AppHandle<App>> {
    HANDLE.get()
}

/// A metrics mark for the next drawn frame (the one that first shows the content). Called on
/// the UI thread only, before that frame is built.
pub fn mark_next(name: &'static str) {
    PENDING_MARKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(name);
}

/// Called from the frame hook after a drawn frame.
pub fn flush_marks() {
    let mut m = PENDING_MARKS.lock().unwrap_or_else(|e| e.into_inner());
    for name in m.drain(..) {
        nt_core::metrics::mark(name);
        // The home page is up: start the player off the UI thread for the first click.
        if name == marks::HOME_LOADED {
            crate::video::prewarm();
        }
    }
}

/// Runs `f` with a job's result on the UI thread, inside a commit.
pub fn on_result<T: Send + 'static>(
    job: Job<T>,
    f: impl FnOnce(&mut App, nt_core::Result<T>, &mut Tx) + Send + 'static,
) {
    job.on_done(move |r| {
        if let Some(h) = handle() {
            h.update(move |s, tx| f(s, r, tx));
        }
    });
}

/// A plain sentence for an error; the raw text goes to the log.
pub fn explain(e: &nt_core::Error) -> String {
    eprintln!("nt-rune: {e}");
    match e {
        nt_core::Error::Network(_) => "Couldn't reach YouTube. Check your connection.".into(),
        nt_core::Error::Fixture(_) => "This page isn't in the recorded data.".into(),
        nt_core::Error::Player(_) => "This video is unavailable.".into(),
        _ => "YouTube sent something this app doesn't understand.".into(),
    }
}

pub const SUGGEST_DEBOUNCE: f64 = 0.15;
/// Pages kept behind the current one (Back) and ahead of it (Forward).
const MAX_BACK: usize = 40;
const MAX_FORWARD: usize = 20;

/// Remembered choices (autoplay, theatre), in `<data dir>/rune-prefs.txt`.
#[derive(Clone, Copy, Debug)]
pub struct Prefs {
    pub autoplay: bool,
    pub theatre: bool,
    /// The full guide (☰) instead of the rail, beside pages other than Watch.
    pub guide: bool,
}

impl Prefs {
    fn path() -> std::path::PathBuf {
        nt_core::data_dir().join("rune-prefs.txt")
    }

    fn load() -> Prefs {
        let text = std::fs::read_to_string(Self::path()).unwrap_or_default();
        let flag = |name: &str, default: bool| {
            text.lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
                .map_or(default, |v| v.trim() == "1")
        };
        Prefs {
            autoplay: flag("autoplay", true),
            theatre: flag("theatre", false),
            guide: flag("guide", false),
        }
    }

    /// One writer preserves the order of rapid Guide and theatre toggles.
    fn save(self) {
        static WRITER: OnceLock<Option<mpsc::Sender<Prefs>>> = OnceLock::new();
        let writer = WRITER.get_or_init(|| {
            let (send, receive) = mpsc::channel::<Prefs>();
            std::thread::Builder::new()
                .name("nt-prefs".into())
                .spawn(move || {
                    while let Ok(prefs) = receive.recv() {
                        receive.try_iter().last().unwrap_or(prefs).write();
                    }
                })
                .ok()
                .map(|_| send)
        });
        if let Some(writer) = writer {
            let _ = writer.send(self);
        }
    }

    fn write(self) {
        let text = format!(
            "autoplay={}\ntheatre={}\nguide={}\n",
            self.autoplay as u8, self.theatre as u8, self.guide as u8
        );
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// A video's menu, from its "⋮" button or a right-click: for what, and where (window points:
/// the pointer for a right-click, the button's corner for "⋮").
pub struct CardMenu {
    pub video: Video,
    pub at: [f32; 2],
    pub at_pointer: bool,
}

pub struct App {
    /// The navigation stack: `root`, then `stack` on top of it.
    root: Page,
    stack: Vec<Page>,
    forward: Vec<Page>,
    watch_frames: std::collections::VecDeque<(u64, rune::Pixels)>,
    mem_trace_due: f64,
    test_cc_done: bool,
    test_fullscreen_done: bool,
    test_cycle_due: f64,
    pub search: TextEdit,
    pub sugg: Vec<String>,
    pub sugg_open: bool,
    pub sugg_sel: Option<usize>,
    /// The prefix the shown suggestions answer.
    pub sugg_for: String,
    sugg_due: Option<f64>,
    sugg_seq: u64,
    /// Below 656 points the search field collapses to a button; this is it expanded.
    pub search_open: bool,
    blur_due: Option<f64>,
    /// The window and the scroll viewport, from layout.
    pub win_w: f32,
    pub win_h: f32,
    pub vw: f32,
    pub vh: f32,
    pub scale: Cell<f32>,
    pub frame_now: f64,
    pub images: Images,
    pub video: VideoState,
    pub preview: crate::preview::Preview,
    #[cfg(target_os = "linux")]
    pub native_video: crate::wayland_video::NativeVideo,
    pub progress: Progress,
    pub prefs: Prefs,
    pub fullscreen: bool,
    pub card_menu: Option<CardMenu>,
    pub toast: Option<(String, f64)>,
    /// The Home chip bar's horizontal offset, its content width and the live translation.
    pub chips_x: f32,
    pub chips_live: Live<[f32; 2]>,
    first_frame: bool,
    /// The player's right-click menu, at this window point.
    pub player_menu: Option<[f32; 2]>,
    /// The guide drawn over a watch page (☰ there).
    pub guide_over: bool,
    /// The `?` keyboard shortcut sheet.
    pub shortcuts: bool,
    pub search_filters_open: bool,
    pub queue_open: bool,
    pub resume: Option<nt_core::LastPlayed>,
    pub probe: RefCell<crate::probe::Probe>,
    pub scene_version: Cell<u64>,
}

pub fn init() -> App {
    let mut app = App {
        root: home_page(0),
        stack: Vec::new(),
        forward: Vec::new(),
        watch_frames: Default::default(),
        mem_trace_due: 0.0,
        test_cc_done: false,
        test_fullscreen_done: false,
        test_cycle_due: 0.0,
        search: TextEdit::new(""),
        sugg: Vec::new(),
        sugg_open: false,
        sugg_sel: None,
        sugg_for: String::new(),
        sugg_due: None,
        sugg_seq: 0,
        search_open: false,
        blur_due: None,
        win_w: 1280.0,
        win_h: 800.0,
        vw: 1208.0,
        vh: 688.0,
        scale: Cell::new(1.0),
        frame_now: 0.0,
        images: Images::default(),
        video: VideoState::default(),
        preview: Default::default(),
        #[cfg(target_os = "linux")]
        native_video: Default::default(),
        progress: Progress::default(),
        prefs: Prefs::load(),
        fullscreen: false,
        card_menu: None,
        toast: None,
        chips_x: 0.0,
        chips_live: Live::new([0.0, 0.0]),
        first_frame: true,
        player_menu: None,
        guide_over: false,
        shortcuts: false,
        search_filters_open: false,
        queue_open: false,
        resume: None,
        probe: RefCell::default(),
        scene_version: Cell::new(0),
    }
    .with_loaded_root();
    let library = library();
    app.resume = library.last_played().filter(|entry| entry.position > 0.0);
    if let Some(entry) = &mut app.resume
        && entry.title.is_empty()
        && let Some(video) = library.history().into_iter().find(|v| v.id == entry.id)
    {
        entry.title = video.title;
    }
    app
}

fn home_page(topic: usize) -> Page {
    Page::new(Kind::Home { topic })
}

impl App {
    fn with_loaded_root(mut self) -> App {
        self.load_top();
        self
    }

    pub fn top(&self) -> &Page {
        self.stack.last().unwrap_or(&self.root)
    }

    pub fn top_mut(&mut self) -> &mut Page {
        match self.stack.last_mut() {
            Some(p) => p,
            None => &mut self.root,
        }
    }

    pub fn can_back(&self) -> bool {
        !self.stack.is_empty()
    }

    pub fn back_depth(&self) -> usize {
        self.stack.len()
    }

    pub fn forward_depth(&self) -> usize {
        self.forward.len()
    }

    /// The width of what is beside the page on the left: the guide, the rail, or nothing (Watch,
    /// narrow windows).
    pub fn side_w(&self) -> f32 {
        if !self.top().has_rail(self.win_w) {
            0.0
        } else if self.guide_wide() {
            crate::theme::GUIDE_W
        } else {
            crate::theme::RAIL
        }
    }

    /// The full guide pushes the page (not on Watch, and only when there is room for it).
    pub fn guide_wide(&self) -> bool {
        self.prefs.guide && self.win_w >= 1000.0 && self.top().has_rail(self.win_w)
    }

    /// ☰: the guide on and off. Beside a page it swaps the rail for the guide (remembered); on
    /// Watch (or a narrow window) it slides over the page.
    pub fn toggle_guide(&mut self) {
        if self.top().has_rail(self.win_w) && self.win_w >= 1000.0 {
            self.prefs.guide = !self.prefs.guide;
            self.prefs.save();
            self.guide_over = false;
        } else {
            self.guide_over = !self.guide_over;
        }
        self.top().touch();
    }

    /// Closes the popups (menus, sheets, the guide over Watch); true when one was open.
    pub fn close_popups(&mut self) -> bool {
        let open = self.card_menu.is_some()
            || self.player_menu.is_some()
            || self.shortcuts
            || self.search_filters_open
            || self.queue_open
            || self.guide_over
            || self.video.menu != Menu::Closed;
        self.card_menu = None;
        self.player_menu = None;
        self.shortcuts = false;
        self.search_filters_open = false;
        self.queue_open = false;
        self.guide_over = false;
        self.video.menu = Menu::Closed;
        self.refresh_controls();
        open
    }

    /// The page with this id, if it is still on the stack and `req` is still its request.
    fn page_for(&mut self, id: u64, req: u64) -> Option<&mut Page> {
        if self.root.id == id {
            return (self.root.req == req).then_some(&mut self.root);
        }
        self.stack
            .iter_mut()
            .chain(self.forward.iter_mut())
            .find(|p| p.id == id)
            .filter(|p| p.req == req)
    }

    fn is_top(&self, id: u64) -> bool {
        self.top().id == id
    }

    /// The geometry the top page is laid out for.
    /// Ready on the GPU path, but its first frame of this video hasn't presented: the poster
    /// stays up with the spinner (never a black hole).
    pub fn gpu_pending(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.native_video.attached()
                && self.video.active
                && self.video.shown.status == nt_core::Status::Ready
                && !self.video.shown.has_picture
                && !self.native_picture()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    pub fn native_picture(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.native_video.showing
                && self.video.active
                && self.video.shown.status != nt_core::Status::Loading
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    pub fn geo(&self) -> Geo {
        Geo {
            win_w: self.win_w,
            win_h: self.win_h,
            vw: if self.fullscreen {
                self.win_w
            } else {
                self.win_w - self.side_w()
            },
            vh: self.win_h
                - crate::theme::TOP_BAR
                - if self.top().has_chips() {
                    crate::theme::CHIP_BAR
                } else {
                    0.0
                },
            theatre: self.prefs.theatre,
            show_resume: self.resume.is_some(),
        }
    }

    // ----- navigation ---------------------------------------------------

    /// Leaving the top page: a watch page stops playback and remembers where it was.
    fn leave_top(&mut self, tx: &mut Tx, continuing_watch: bool) {
        tx.instant();
        self.preview.cancel();
        #[cfg(target_os = "linux")]
        self.native_video.hide();
        let scroller = &mut self.top_mut().scroll;
        scroller.anim.jump_to(scroller.anim.offset());
        let paused = self.video.shown.paused;
        let duration = self.video.duration;
        let picture = self
            .video
            .shown
            .has_picture
            .then(|| self.video.picture.clone());
        if self.top().watch().is_some() {
            if let (Some(f), Some(id)) = (
                self.video.played_fraction(),
                self.top().watch().map(|w| w.id.clone()),
            ) {
                self.progress.set(&id, f);
            }
            let pos = self.video.leave(continuing_watch);
            if let Some(w) = self.top_mut().watch_mut() {
                w.resume_at = pos;
                w.resume_paused = paused;
                w.resume_duration = duration;
                if let Some(picture) = picture {
                    let page = self.top().id;
                    self.watch_frames.retain(|(id, _)| *id != page);
                    self.watch_frames.push_back((page, picture));
                    if self.watch_frames.len() > 3 {
                        self.watch_frames.pop_front();
                    }
                }
            }
            if self.fullscreen {
                self.fullscreen = false;
                tx.toggle_full_screen();
            }
        }
        self.video.pointer_in = false;
        self.video.stats_open = false;
        self.close_popups();
        self.close_suggestions();
        self.search_open = false;
    }

    /// The top page came back into view (Back, Forward): its video resumes where it stopped;
    /// library pages show the library as it is now.
    fn enter_top(&mut self) {
        if let Some((id, at, paused, duration)) = self.top().watch().map(|w| {
            (
                w.id.clone(),
                w.resume_at,
                w.resume_paused,
                w.resume_duration,
            )
        }) {
            self.video.play(&id, at, paused, duration);
            self.video.poster_frame = self
                .watch_frames
                .iter()
                .find(|(id, _)| *id == self.top().id)
                .map(|(_, picture)| picture.clone());
            let marks = self
                .top()
                .watch()
                .and_then(|w| w.info.as_ref())
                .map(|i| (i.chapters.clone(), i.heatmap.clone()));
            if let Some((chapters, heat)) = marks {
                self.video.set_watch_info(&id, &chapters, heat);
            }
        }
        if let Kind::Subs { .. } | Kind::History | Kind::Later = self.top().kind {
            self.refresh_library_page();
        }
        // A results page brings its query back into the search box.
        if let Kind::Search(sr) = &self.top().kind
            && self.search.text != sr.query
        {
            let q = sr.query.clone();
            self.search.replace_text(q);
            let end = self.search.text.len();
            self.search.set_caret(end);
        }
    }

    pub fn push(&mut self, kind: Kind, tx: &mut Tx) {
        self.push_page(Page::new(kind), tx);
    }

    fn push_page(&mut self, page: Page, tx: &mut Tx) {
        self.leave_top(tx, page.watch().is_some());
        self.forward.clear();
        // Keep the root; drop the oldest pages above it once the stack is deep.
        if self.stack.len() >= MAX_BACK {
            self.stack.remove(0);
        }
        self.stack.push(page);
        self.load_top();
    }

    pub fn root(&mut self, kind: Kind, tx: &mut Tx) {
        self.leave_top(tx, false);
        self.stack.clear();
        self.forward.clear();
        self.root = Page::new(kind);
        self.load_top();
    }

    pub fn back(&mut self, tx: &mut Tx) {
        if self.stack.is_empty() {
            return;
        }
        let continuing_watch = self
            .stack
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|p| p.watch().is_some());
        self.leave_top(tx, continuing_watch);
        if let Some(p) = self.stack.pop() {
            if self.forward.len() >= MAX_FORWARD {
                self.forward.remove(0);
            }
            self.forward.push(p);
        }
        self.enter_top();
    }

    pub fn go_forward(&mut self, tx: &mut Tx) {
        let Some(p) = self.forward.pop() else { return };
        self.leave_top(tx, p.watch().is_some());
        self.stack.push(p);
        self.enter_top();
    }

    pub fn resume_last(&mut self, tx: &mut Tx) {
        let Some(saved) = self.resume.take() else {
            return;
        };
        let video = library()
            .history()
            .into_iter()
            .find(|v| v.id == saved.id)
            .unwrap_or_else(|| Video {
                thumbnail: Video::thumbnail_url(&saved.id),
                id: saved.id,
                title: saved.title,
                ..Video::default()
            });
        self.open_video_at(video, Some(saved.position), tx);
    }

    pub fn open_video(&mut self, v: Video, tx: &mut Tx) {
        let start = v.moment.as_ref().map(|moment| moment.start_secs);
        self.open_video_at(v, start, tx);
    }

    /// Opens a video, from `start` seconds (a transcript match) or the beginning.
    pub fn open_video_at(&mut self, v: Video, start: Option<f64>, tx: &mut Tx) {
        self.open_watch(v, start, None, tx);
    }

    pub fn open_watch(
        &mut self,
        v: Video,
        start: Option<f64>,
        panel: Option<nt_core::PlaylistPanel>,
        tx: &mut Tx,
    ) {
        library().record_watch(&v);
        let id = v.id.clone();
        let mut watch = Watch::new(v);
        watch.playlist_key = panel.as_ref().map(|panel| panel.id.clone());
        watch.playlist = panel;
        self.push(Kind::Watch(Box::new(watch)), tx);
        self.video.play(&id, start, false, 0.0);
    }

    pub fn open_playlist(&mut self, id: String) {
        let request = self.top_mut().begin_playlist_open();
        on_result(client().playlist(&id), move |s, result, tx| {
            if !s.top().accepts_playlist_open(request) {
                return;
            }
            match result {
                Ok(mut panel) => {
                    let index = panel.index.unwrap_or(0);
                    if let Some(video) = panel.items.get(index).cloned() {
                        panel.index = Some(index);
                        s.open_watch(video, None, Some(panel), tx);
                    } else {
                        s.show_toast("This playlist has no videos", tx);
                    }
                }
                Err(error) => s.show_toast(&explain(&error), tx),
            }
        });
    }

    pub fn play_playlist_item(&mut self, index: usize, tx: &mut Tx) {
        let panel = self.top().watch().and_then(|w| w.playlist.clone());
        if let Some(mut panel) = panel
            && let Some(video) = panel.items.get(index).cloned()
        {
            panel.index = Some(index);
            self.open_watch(video, None, Some(panel), tx);
        }
    }

    pub fn load_playlist_more(&mut self) {
        let page = self.top_mut();
        let (id, req) = (page.id, page.req);
        let Some(w) = page.watch_mut() else { return };
        let Some(next) = w
            .playlist
            .as_ref()
            .and_then(|panel| panel.next.clone())
            .filter(|_| w.playlist_more != Load::Loading)
        else {
            return;
        };
        w.playlist_more = Load::Loading;
        page.touch();
        on_result(client().more_playlist_videos(&next), move |s, result, _| {
            let Some(page) = s.page_for(id, req) else {
                return;
            };
            let Some(w) = page.watch_mut() else { return };
            match result {
                Ok(more) => {
                    if let Some(panel) = &mut w.playlist {
                        panel.items.extend(more.items);
                        panel.next = more.next.filter(|cursor| cursor != &next);
                    }
                    w.playlist_more = Load::Idle;
                }
                Err(error) => w.playlist_more = Load::Error(explain(&error)),
            }
            page.touch();
        });
    }

    pub fn open_channel(&mut self, id: String, tx: &mut Tx) {
        if id.is_empty() {
            return;
        }
        self.push(
            Kind::Channel {
                id,
                info: None,
                desc_open: false,
            },
            tx,
        );
    }

    pub fn channel_tab(&mut self, tab: nt_core::ChannelTab, tx: &mut Tx) {
        let Kind::Channel { id, info, .. } = &self.top().kind else {
            return;
        };
        if self.top().channel_tab == Some(tab) {
            return;
        }
        let kind = Kind::Channel {
            id: id.clone(),
            info: info.clone(),
            desc_open: false,
        };
        let mut page = Page::new(kind);
        page.channel_tab = Some(tab);
        self.push_page(page, tx);
    }

    pub fn sort_channel(&mut self, index: usize) {
        let page = self.top_mut();
        let Kind::Channel {
            info: Some(info), ..
        } = &page.kind
        else {
            return;
        };
        if page.channel_sort == Some(index) || page.feed.more == Load::Loading {
            return;
        }
        let Some(chip) = info.video_sort.get(index) else {
            return;
        };
        let next = chip.next.clone();
        if next.is_none() {
            page.feed
                .set(info.videos.items.clone(), info.videos.next.clone());
            page.channel_sort = Some(index);
            page.pending_sort = None;
            page.touch();
            return;
        }
        let Some(next) = next else { return };
        let (id, req) = (page.id, page.req);
        page.pending_sort = Some(index);
        page.feed.more = Load::Loading;
        page.touch();
        on_result(client().more_videos(&next), move |s, result, _| {
            let Some(page) = s.page_for(id, req) else {
                return;
            };
            match result {
                Ok(list) => {
                    page.channel_sort = Some(index);
                    page.pending_sort = None;
                    page.feed.set(list.items, list.next);
                }
                Err(error) => page.feed.more = Load::Error(explain(&error)),
            }
            page.touch();
        });
    }

    pub fn load_playlist_cards(&mut self) {
        let page = self.top_mut();
        let Kind::Channel {
            info: Some(info), ..
        } = &page.kind
        else {
            return;
        };
        let Some(next) = info
            .playlists
            .next
            .clone()
            .filter(|_| page.playlist_cards_more != Load::Loading)
        else {
            return;
        };
        let (id, req) = (page.id, page.req);
        page.playlist_cards_more = Load::Loading;
        page.touch();
        on_result(client().more_playlists(&next), move |s, result, _| {
            let Some(page) = s.page_for(id, req) else {
                return;
            };
            let Kind::Channel {
                info: Some(info), ..
            } = &mut page.kind
            else {
                return;
            };
            match result {
                Ok(more) => {
                    info.playlists.items.extend(more.items);
                    info.playlists.next = more.next.filter(|cursor| cursor != &next);
                    page.playlist_cards_more = Load::Idle;
                }
                Err(error) => page.playlist_cards_more = Load::Error(explain(&error)),
            }
            page.touch();
        });
    }

    pub fn channel_about(&mut self, tx: &mut Tx) {
        let page = self.top_mut();
        let Kind::Channel {
            info: Some(info),
            desc_open,
            ..
        } = &mut page.kind
        else {
            return;
        };
        *desc_open = !*desc_open;
        let cursor = (*desc_open).then(|| info.about.clone()).flatten();
        let (id, req) = (page.id, page.req);
        page.touch();
        tx.instant();
        if let Some(cursor) = cursor {
            on_result(client().channel_about(&cursor), move |s, result, tx| {
                let Some(page) = s.page_for(id, req) else {
                    return;
                };
                if let Kind::Channel {
                    info: Some(info), ..
                } = &mut page.kind
                {
                    match result {
                        Ok(about) => {
                            if !about.description.is_empty() {
                                info.description = about.description;
                            }
                            info.links = about.links;
                            info.about = None;
                        }
                        Err(error) => {
                            if s.is_top(id) {
                                s.show_toast(&explain(&error), tx);
                            }
                            return;
                        }
                    }
                }
                page.touch();
            });
        }
    }

    pub fn submit_search(&mut self, tx: &mut Tx) {
        let q = self.search.text.trim().to_string();
        self.close_suggestions();
        if q.is_empty() {
            return;
        }
        tx.blur();
        self.search_open = false;
        self.push(Kind::Search(Box::new(Search::new(q))), tx);
    }

    pub fn search_filter(&mut self, filter: nt_core::SearchFilter, tx: &mut Tx) {
        self.search_filters_open = false;
        let Kind::Search(current) = &self.top().kind else {
            return;
        };
        if current.filter == filter {
            return;
        }
        let mut search = Search::new(current.query.clone());
        search.chips.clone_from(&current.chips);
        search.filters.clone_from(&current.filters);
        for option in search.chips.iter_mut().chain(
            search
                .filters
                .iter_mut()
                .flat_map(|group| &mut group.options),
        ) {
            option.selected = option.filter.as_ref() == Some(&filter);
        }
        search.filter = filter;
        self.search.replace_text(search.query.clone());
        self.push(Kind::Search(Box::new(search)), tx);
    }

    // ----- loading ------------------------------------------------------

    /// Starts (or restarts) the top page's data load under a new request token. A watch page
    /// loads its info and related list only; playback is started by `open_video` and Back, so
    /// retrying page data never restarts the video.
    pub fn load_top(&mut self) {
        if matches!(self.top().kind, Kind::History | Kind::Later) {
            self.refresh_library_page();
            return;
        }
        let page = self.top_mut();
        page.req += 1;
        let (id, req) = (page.id, page.req);
        page.feed.reset();
        page.touch();
        match &mut page.kind {
            Kind::Home { topic } => {
                let selected = page
                    .home_chip
                    .and_then(|i| page.home_chips.get(i))
                    .and_then(|chip| chip.next.clone());
                if let Some(next) = selected {
                    feed_job(id, req, client().more_videos(&next), marks::HOME_LOADED);
                } else if *topic == 0 {
                    on_result(client().home_feed(), move |s, r, _| {
                        let top = s.is_top(id);
                        let Some(page) = s.page_for(id, req) else {
                            return;
                        };
                        match r {
                            Ok(feed) => {
                                page.home_chip = feed.chips.iter().position(|chip| chip.selected);
                                page.home_chips = feed.chips;
                                page.feed.set(feed.videos.items, feed.videos.next);
                                if top {
                                    mark_next(marks::HOME_LOADED);
                                }
                            }
                            Err(error) => page.feed.status = Load::Error(explain(&error)),
                        }
                        page.touch();
                    });
                } else {
                    let topic = nt_core::TOPICS.get(*topic).map_or("all", |t| t.id);
                    feed_job(id, req, client().home(topic), marks::HOME_LOADED);
                }
            }
            Kind::Search(sr) => {
                sr.notices.clear();
                sr.trans = Feed::idle();
                sr.trans_total = None;
                let job = client().search_filtered(&sr.query, &sr.filter);
                on_result(job, move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_for(id, req) else { return };
                    match r {
                        Ok(sp) => {
                            if let Kind::Search(sr) = &mut p.kind {
                                sr.append_entries(sp.entries, &sp.results.items);
                                sr.notices = sp.notices;
                                sr.trans.set(sp.transcripts.items, sp.transcripts.next);
                                sr.trans_total = sp.transcript_total;
                                if !sp.chips.is_empty() {
                                    sr.chips = sp.chips;
                                }
                                if !sp.filters.is_empty() {
                                    sr.filters = sp.filters;
                                }
                            }
                            p.feed.set(sp.results.items, sp.results.next);
                            if top {
                                mark_next(marks::SEARCH_LOADED);
                            }
                        }
                        Err(e) => p.feed.status = Load::Error(explain(&e)),
                    }
                    p.touch();
                });
            }
            Kind::Channel { id: cid, .. } => {
                let job = page.channel_tab.map_or_else(
                    || client().channel(cid),
                    |tab| client().channel_tab(cid, tab),
                );
                on_result(job, move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_for(id, req) else { return };
                    match r {
                        Ok(info) => {
                            p.channel_tab = info.selected_tab.or(p.channel_tab);
                            let mut videos = info.videos.items.clone();
                            if let Some(featured) = &info.featured
                                && p.channel_tab == Some(nt_core::ChannelTab::Home)
                            {
                                videos.insert(0, featured.clone());
                            }
                            p.feed.set(videos, info.videos.next.clone());
                            if let Kind::Channel { info: slot, .. } = &mut p.kind {
                                *slot = Some(Box::new(info));
                            }
                            if top {
                                mark_next(marks::CHANNEL_LOADED);
                            }
                        }
                        Err(e) => p.feed.status = Load::Error(explain(&e)),
                    }
                    p.touch();
                });
            }
            Kind::Subs { channels } => {
                let subs = library().subscriptions();
                channels.clone_from(&subs);
                if subs.is_empty() {
                    page.feed.status = Load::Idle;
                } else {
                    on_result(client().subscription_feed(subs), move |s, r, _| {
                        let Some(p) = s.page_for(id, req) else { return };
                        match r {
                            Ok(items) => p.feed.set(items, None),
                            Err(e) => p.feed.status = Load::Error(explain(&e)),
                        }
                        p.touch();
                    });
                }
            }
            Kind::History | Kind::Later => {}
            Kind::Watch(w) => {
                w.error = None;
                let vid = w.id.clone();
                let job = w.playlist_key.as_ref().map_or_else(
                    || client().watch(&vid),
                    |key| client().watch_in_playlist(&vid, key),
                );
                on_result(job, move |s, r, _| {
                    let top = s.is_top(id);
                    if top
                        && s.page_for(id, req).is_some()
                        && let Ok(info) = &r
                    {
                        s.video
                            .set_watch_info(&vid, &info.chapters, info.heatmap.clone());
                    }
                    let Some(p) = s.page_for(id, req) else { return };
                    match r {
                        Ok(info) => {
                            p.feed
                                .set(info.related.items.clone(), info.related.next.clone());
                            if let Some(w) = p.watch_mut() {
                                if let Some(panel) = &info.playlist
                                    && w.playlist
                                        .as_ref()
                                        .is_none_or(|old| panel.items.len() >= old.items.len())
                                {
                                    w.playlist = Some(panel.clone());
                                }
                                w.info = Some(info);
                                w.error = None;
                                library().record_watch(&w.as_video());
                            }
                            if top {
                                mark_next(marks::WATCH_LOADED);
                            }
                        }
                        Err(e) => {
                            p.feed.status = Load::Idle;
                            if let Some(w) = p.watch_mut() {
                                w.error = Some(explain(&e));
                            }
                        }
                    }
                    p.touch();
                });
            }
        }
    }

    /// History and Watch Later show the library (newest first).
    pub fn refresh_library_page(&mut self) {
        let items = match &self.top().kind {
            Kind::History => library().history(),
            Kind::Later => library().watch_later(),
            Kind::Subs { channels } => {
                if *channels != library().subscriptions() {
                    self.load_top();
                }
                return;
            }
            _ => return,
        };
        let p = self.top_mut();
        p.req += 1;
        p.feed.set(items, None);
        p.touch();
    }

    /// The next page of the top page's list (infinite scroll). One request at a time.
    pub fn load_more(&mut self) {
        // A results page whose results ended goes on with its transcript matches.
        let page = self.top_mut();
        if page.feed.next.is_none()
            && page.feed.status == Load::Idle
            && let Kind::Search(sr) = &mut page.kind
        {
            let (id, req) = (page.id, page.req);
            let Some(next) = sr.trans.next.clone() else {
                return;
            };
            if sr.trans.more != Load::Idle {
                return;
            }
            sr.trans.more = Load::Loading;
            page.touch();
            on_result(client().more_videos(&next), move |s, r, _| {
                let top = s.is_top(id);
                let Some(p) = s.page_for(id, req) else { return };
                if let Kind::Search(sr) = &mut p.kind {
                    match r {
                        Ok(more) => {
                            let added = sr.trans.append(more.items);
                            sr.trans.next = more.next.filter(|cursor| cursor != &next);
                            sr.trans.more = Load::Idle;
                            if top && added > 0 {
                                mark_next(marks::MORE_LOADED);
                            }
                        }
                        Err(e) => sr.trans.more = Load::Error(explain(&e)),
                    }
                }
                p.touch();
            });
            return;
        }
        if page.feed.status != Load::Idle || page.feed.more != Load::Idle {
            return;
        }
        let Some(next) = page.feed.next.clone() else {
            return;
        };
        let (id, req) = (page.id, page.req);
        page.feed.more = Load::Loading;
        page.touch();
        if matches!(page.kind, Kind::Search(_)) {
            on_result(client().more_search(&next), move |s, reply, _| {
                let top = s.is_top(id);
                let Some(p) = s.page_for(id, req) else { return };
                match reply {
                    Ok(data) => {
                        if let Kind::Search(sr) = &mut p.kind {
                            sr.append_entries(data.entries, &data.results.items);
                        }
                        p.feed.append(data.results.items);
                        p.feed.next = data.results.next.filter(|cursor| cursor != &next);
                        p.feed.more = Load::Idle;
                        if top {
                            mark_next(marks::MORE_LOADED);
                        }
                    }
                    Err(error) => p.feed.more = Load::Error(explain(&error)),
                }
                p.touch();
            });
            return;
        }
        on_result(client().more_videos(&next), move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_for(id, req) else { return };
            match r {
                Ok(more) => {
                    let added = p.feed.append(more.items);
                    p.feed.next = more.next.filter(|cursor| cursor != &next);
                    p.feed.more = Load::Idle;
                    if top && added > 0 {
                        mark_next(marks::MORE_LOADED);
                    }
                }
                Err(e) => p.feed.more = Load::Error(explain(&e)),
            }
            p.touch();
        });
    }

    /// The next page of comments on the watch page (the first if none are loaded).
    pub fn load_comments(&mut self) {
        let page = self.top_mut();
        let (id, req) = (page.id, page.req);
        let Some(w) = page.watch_mut() else { return };
        if w.comments_state == Load::Loading {
            return;
        }
        let order = w.pending_order;
        let cont = if let Some(order) = order {
            w.comments_sort.get(order).map(|sort| sort.next.clone())
        } else if !w.comments_started {
            w.info.as_ref().and_then(|i| i.comments.clone())
        } else {
            w.comments_next.clone()
        };
        let Some(cont) = cont else { return };
        w.comments_state = Load::Loading;
        page.touch();
        on_result(client().more_comments(&cont), move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_for(id, req) else { return };
            if let Some(w) = p.watch_mut() {
                match r {
                    Ok(c) => {
                        if let Some(order) = order {
                            w.comments.clear();
                            w.comment_h.get_mut().clear();
                            w.replies.clear();
                            w.comment_order = Some(order);
                            w.pending_order = None;
                        }
                        if !c.sort.is_empty() && order.is_none() {
                            w.comment_order = c.sort.iter().position(|entry| entry.selected);
                            w.comments_sort = c.sort;
                        }
                        w.comments_started = true;
                        w.comments.extend(c.items);
                        w.comments_next = c.next.filter(|cursor| cursor != &cont);
                        if c.total.is_some() {
                            w.comments_total = c.total;
                        }
                        w.comments_state = Load::Idle;
                        if top {
                            mark_next(marks::COMMENTS_LOADED);
                        }
                    }
                    Err(e) => w.comments_state = Load::Error(explain(&e)),
                }
            }
            p.touch();
        });
    }

    pub fn sort_comments(&mut self, order: usize) {
        if let Some(w) = self.top_mut().watch_mut()
            && w.comment_order != Some(order)
            && w.comments_state != Load::Loading
        {
            w.pending_order = Some(order);
            self.load_comments();
        }
    }

    pub fn toggle_replies(&mut self, comment: String) {
        let Some(w) = self.top_mut().watch_mut() else {
            return;
        };
        if let Some(thread) = w.replies.get_mut(&comment) {
            thread.open = !thread.open;
            self.top().touch();
            return;
        }
        let next = w
            .comments
            .iter()
            .find(|c| c.id == comment)
            .and_then(|c| c.replies.clone());
        let Some(next) = next else { return };
        w.replies.insert(
            comment.clone(),
            Replies {
                open: true,
                items: Vec::new(),
                next: Some(next),
                state: Load::Idle,
            },
        );
        self.load_replies(comment);
    }

    pub fn load_replies(&mut self, comment: String) {
        let page = self.top_mut();
        let (id, req) = (page.id, page.req);
        let Some(thread) = page.watch_mut().and_then(|w| w.replies.get_mut(&comment)) else {
            return;
        };
        let Some(next) = thread
            .next
            .clone()
            .filter(|_| thread.state != Load::Loading)
        else {
            return;
        };
        thread.state = Load::Loading;
        page.touch();
        on_result(client().more_replies(&next), move |s, r, _| {
            let Some(page) = s.page_for(id, req) else {
                return;
            };
            let Some(thread) = page.watch_mut().and_then(|w| w.replies.get_mut(&comment)) else {
                return;
            };
            if thread.next.as_ref() != Some(&next) || thread.state != Load::Loading {
                return;
            }
            match r {
                Ok(more) => {
                    thread.items.extend(more.items);
                    thread.next = more.next.filter(|cursor| cursor != &next);
                    thread.state = Load::Idle;
                }
                Err(error) => thread.state = Load::Error(explain(&error)),
            }
            page.touch();
        });
    }

    pub fn related_chip(&mut self, index: usize) {
        let page = self.top_mut();
        if page.feed.more == Load::Loading {
            return;
        }
        let Some(w) = page.watch_mut() else { return };
        let Some(info) = &w.info else { return };
        let Some(chip) = info.related_chips.get(index) else {
            return;
        };
        let next = chip.next.clone();
        if next.is_none() {
            let related = info.related.clone();
            w.pending_related = None;
            w.related_chip = Some(index);
            page.feed.set(related.items, related.next);
            page.touch();
            return;
        }
        let Some(next) = next else { return };
        w.pending_related = Some(index);
        let (id, req) = (page.id, page.req);
        page.feed.more = Load::Loading;
        page.touch();
        on_result(client().more_videos(&next), move |s, result, _| {
            let Some(page) = s.page_for(id, req) else {
                return;
            };
            match result {
                Ok(found) => {
                    if let Some(w) = page.watch_mut() {
                        w.related_chip = Some(index);
                        w.pending_related = None;
                    }
                    page.feed.set(found.items, found.next);
                }
                Err(error) => page.feed.more = Load::Error(explain(&error)),
            }
            page.touch();
        });
    }

    pub fn retry_more(&mut self) {
        let page = self.top_mut();
        if !matches!(page.feed.more, Load::Error(_)) {
            return;
        }
        page.feed.more = Load::Idle;
        if let Some(index) = page.watch().and_then(|w| w.pending_related) {
            self.related_chip(index);
        } else if let Some(index) = page.pending_sort {
            self.sort_channel(index);
        } else {
            self.load_more();
        }
    }

    pub fn retry_transcripts(&mut self) {
        if let Kind::Search(sr) = &mut self.top_mut().kind
            && matches!(sr.trans.more, Load::Error(_))
        {
            sr.trans.more = Load::Idle;
            self.load_more();
        }
    }

    pub fn set_home_chip(&mut self, index: usize) {
        let page = self.top_mut();
        if page.home_chip != Some(index) && page.home_chips.get(index).is_some() {
            page.home_chip = Some(index);
            page.scroll.anim.jump_to(0.0);
            self.load_top();
        }
    }

    pub fn set_topic(&mut self, topic: usize) {
        let page = self.top_mut();
        if let Kind::Home { topic: t } = &mut page.kind {
            if *t == topic && page.feed.status != Load::Idle {
                return;
            }
            *t = topic;
            page.scroll.anim.jump_to(0.0);
            self.load_top();
        }
    }

    // ----- suggestions --------------------------------------------------

    pub fn search_changed(&mut self, tx: &mut Tx) {
        // Any answer still in flight is for an older prefix now.
        self.sugg_seq += 1;
        self.sugg_due = Some(now() + SUGGEST_DEBOUNCE);
        self.sugg_sel = None;
        tx.wake_after(Duration::from_secs_f64(SUGGEST_DEBOUNCE));
    }

    fn fetch_suggestions(&mut self) {
        let q = self.search.text.trim().to_string();
        self.sugg_seq += 1;
        if q.is_empty() {
            self.close_suggestions();
            return;
        }
        let seq = self.sugg_seq;
        on_result(client().suggestions(&q), move |s, r, _| {
            if s.sugg_seq != seq || !s.search.focused {
                return;
            }
            let mut items = r.unwrap_or_default();
            items.truncate(10);
            s.sugg_open = !items.is_empty();
            s.sugg = items;
            s.sugg_for = q;
            s.sugg_sel = None;
        });
    }

    pub fn close_suggestions(&mut self) {
        self.sugg_open = false;
        self.sugg_sel = None;
        self.sugg_due = None;
        self.sugg_seq += 1;
    }

    pub fn choose_suggestion(&mut self, i: usize, tx: &mut Tx) {
        if let Some(q) = self.sugg.get(i).cloned() {
            self.search.replace_text(q);
            let end = self.search.text.len();
            self.search.set_caret(end);
            self.submit_search(tx);
        }
    }

    /// Keys in the search field before it edits: the dropdown's Up/Down/Enter/Escape.
    pub fn search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if is_quit(ev) {
            tx.quit();
            return true;
        }
        let n = self.sugg.len();
        match ev.key {
            Key::Down if self.sugg_open && n > 0 => {
                self.sugg_sel = Some(self.sugg_sel.map_or(0, |i| (i + 1) % n));
                true
            }
            Key::Up if self.sugg_open && n > 0 => {
                self.sugg_sel = Some(self.sugg_sel.map_or(n - 1, |i| (i + n - 1) % n));
                true
            }
            Key::Enter => {
                match self.sugg_sel.filter(|_| self.sugg_open) {
                    Some(i) => self.choose_suggestion(i, tx),
                    None => self.submit_search(tx),
                }
                true
            }
            Key::Escape => {
                if self.sugg_open {
                    self.close_suggestions();
                } else {
                    tx.blur();
                    self.search_open = false;
                }
                true
            }
            _ => false,
        }
    }

    // ----- scrolling input ----------------------------------------------

    /// Mouse-wheel notches (positive scrolls down): `wheel_ticks`.
    pub fn wheel_ticks(&mut self, ticks: f64, t: f64) {
        let now = self.frame_now;
        let s = &mut self.top_mut().scroll;
        s.anim.wheel_ticks(ticks, t);
        // The frame being built already shows the start of the motion.
        s.present(now);
    }

    /// Precise touchpad deltas in px (positive down): `wheel_pixels`.
    pub fn wheel_pixels(&mut self, px: f64, t: f64) {
        let now = self.frame_now;
        let s = &mut self.top_mut().scroll;
        s.anim.wheel_pixels(px, t);
        s.present(now);
    }

    pub fn fling_start(&mut self, t: f64) {
        let now = self.frame_now;
        let s = &mut self.top_mut().scroll;
        s.anim.fling_start(t);
        s.present(now);
    }

    /// The chip bar's horizontal scroll, clamped to its overflow.
    pub fn scroll_chips(&mut self, dx: f32) {
        let room =
            (crate::view::chips_width(self.top()) - (self.vw - 2.0 * self.geo().pad())).max(0.0);
        self.chips_x = (self.chips_x + dx).clamp(0.0, room);
        self.chips_live.set([-self.chips_x, 0.0]);
    }

    pub fn on_wheel(&mut self, e: WheelEvent, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
        let preview_closed = self.preview.cancel();
        // A scroll closes an open menu (as YouTube's do); that one event needs a view run.
        if self.card_menu.take().is_some() | self.player_menu.take().is_some() {
            tx.instant();
            return true;
        }
        if self.search_filters_open || self.queue_open {
            return false;
        }
        if self.video.menu != Menu::Closed {
            if self.video.menu == Menu::Main {
                self.video.menu = Menu::Closed;
                self.refresh_controls();
            }
            return false;
        }
        // Horizontal wheel or Shift+wheel over the Home chip bar scrolls the chips.
        let rail = self.side_w();
        let over_chips = self.top().has_chips()
            && e.y >= crate::theme::TOP_BAR
            && e.y < crate::theme::TOP_BAR + crate::theme::CHIP_BAR
            && e.x >= rail;
        if over_chips && e.dx != 0.0 {
            self.scroll_chips(-e.dx);
            return true;
        }
        // Over a sheet (shortcuts) or the guide drawn over the page, the page doesn't scroll.
        if self.fullscreen
            || self.shortcuts
            || self.guide_over
            || self.search_filters_open
            || e.x < rail
            || e.y < crate::theme::TOP_BAR
        {
            return true;
        }
        if e.precise {
            // winit (Wayland) passes the raw axis value as a logical PixelDelta with its sign
            // flipped; Rune divides by the scale. Chrome scales it ×12 (`wayland_axis_to_px`).
            if e.dy != 0.0 {
                self.wheel_pixels(
                    nt_core::scroll::wayland_axis_to_px(-e.dy as f64) * touchpad_factor(),
                    t,
                );
            }
            if e.phase == GesturePhase::Ended {
                self.fling_start(t);
            }
        } else if e.dy != 0.0 {
            // A notch arrives as LineDelta(±1), which Rune reports as ±40 points.
            self.wheel_ticks(-e.dy as f64 / 40.0, t);
        }
        if !preview_closed {
            tx.live_only();
        }
        tx.wake_after(Duration::ZERO);
        true
    }

    /// A press on content while the page flings stops the fling and does nothing else, as a
    /// tap on a flinging page does in Chrome. Returns whether the press may act.
    pub fn press_ok(&mut self, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
        let s = &mut self.top_mut().scroll;
        if s.anim.velocity().abs() > 300.0 {
            s.anim.cancel_fling(t);
            return false;
        }
        true
    }

    pub fn cancel_fling(&mut self, tx: &mut Tx) {
        let t = at(tx.input_time());
        self.top_mut().scroll.anim.cancel_fling(t);
    }

    fn scroll_key(&mut self, key: &Key, shift: bool, t: f64) -> bool {
        let s = &mut self.top_mut().scroll;
        let vh = s.viewport() as f64;
        let a = &mut s.anim;
        match key {
            Key::Up => a.scroll_by(-a.line_step(), t),
            Key::Down => a.scroll_by(a.line_step(), t),
            Key::PageUp => a.scroll_by(-a.page_step(vh), t),
            Key::PageDown => a.scroll_by(a.page_step(vh), t),
            Key::Text(c) if c == " " => {
                let step = a.page_step(vh);
                a.scroll_by(if shift { -step } else { step }, t)
            }
            Key::Home => a.scroll_by(-a.document_step(), t),
            Key::End => a.scroll_by(a.document_step(), t),
            _ => return false,
        }
        true
    }

    // ----- watch page -----------------------------------------------------

    pub fn toggle_fullscreen(&mut self, tx: &mut Tx) {
        if self.top().watch().is_none() && !self.fullscreen {
            return;
        }
        self.fullscreen = !self.fullscreen;
        self.video.menu = Menu::Closed;
        tx.toggle_full_screen();
        self.top().touch();
    }

    pub fn toggle_theatre(&mut self) {
        self.prefs.theatre = !self.prefs.theatre;
        self.prefs.save();
        self.top().touch();
    }

    pub fn toggle_autoplay(&mut self) {
        self.prefs.autoplay = !self.prefs.autoplay;
        self.prefs.save();
        if !self.prefs.autoplay {
            self.video.up_next = None;
        }
    }

    /// A click on the picture: pause or play, with the center bezel.
    pub fn click_player(&mut self) {
        if self.video.menu != Menu::Closed {
            self.video.menu = Menu::Closed;
            return;
        }
        let will_play = !self.video.shown.playing();
        self.video.toggle_pause();
        self.video.flash_bezel(will_play);
    }

    /// The pointer moved over the player: show the controls and the cursor for 3 s.
    pub fn player_moved(&mut self, tx: &mut Tx) {
        let now = now();
        self.video.pointer_in = true;
        self.video.last_move = now;
        if self.video.hide_due.is_none() {
            self.video.hide_due = Some(now + crate::video::CONTROLS_HIDE);
            tx.wake_after(Duration::from_secs_f64(crate::video::CONTROLS_HIDE));
        }
        if self.refresh_controls() {
            tx.instant();
        } else {
            tx.live_only();
        }
    }

    pub fn player_hover(&mut self, on: bool, tx: &mut Tx) {
        self.video.pointer_in = on;
        if on {
            self.player_moved(tx);
        } else if self.refresh_controls() {
            tx.instant();
        }
    }

    /// Recomputes whether the controls show; true when that changed.
    pub fn refresh_controls(&mut self) -> bool {
        let want = self.video.want_controls(now());
        if want != self.video.controls {
            self.video.controls = want;
            return true;
        }
        false
    }

    /// Opens a video that is next in line (Up next, or a related row).
    pub fn next_video(&self) -> Option<Video> {
        library()
            .queue()
            .into_iter()
            .next()
            .or_else(|| {
                self.top()
                    .watch()
                    .and_then(|w| w.playlist.as_ref())
                    .and_then(|p| p.next_video().cloned())
            })
            .or_else(|| self.top().feed.items.first().cloned())
    }

    pub fn play_next(&mut self, tx: &mut Tx) {
        if let Some(video) = library().pop_queued() {
            self.open_video(video, tx);
            return;
        }
        let panel = self.top().watch().and_then(|w| w.playlist.as_ref());
        if let Some(index) = panel.and_then(|p| p.index).and_then(|i| i.checked_add(1))
            && panel.is_some_and(|p| p.items.get(index).is_some())
        {
            self.play_playlist_item(index, tx);
        } else if let Some(video) = self.top().feed.items.first().cloned() {
            self.open_video(video, tx);
        }
    }

    /// The sleep timer: `minutes` from now, or None for off.
    pub fn set_sleep(&mut self, sleep: crate::video::Sleep, tx: &mut Tx) {
        if sleep == crate::video::Sleep::EndOfVideo && self.video.shown.looping {
            self.video.toggle_loop();
        }
        self.video.sleep = sleep;
        if let crate::video::Sleep::At(due, _) = sleep {
            tx.wake_after(Duration::from_secs_f64((due - now()).max(0.0)));
        }
    }

    pub fn cancel_up_next(&mut self) {
        self.video.up_next = None;
        self.video.up_next_cancelled = true;
    }

    /// The player's state changed (waker): live values, and a view run when something shown
    /// changed.
    pub fn player_changed(&mut self, tx: &mut Tx) -> bool {
        let (mut commit, ended) = self.video.sync();
        // The GPU worker wakes us when its first frame presents. Nothing else may run a frame
        // after that (the picture itself never wakes Rune), so open the hole from here; waiting
        // for `poll` left the poster up for as long as nothing else changed.
        #[cfg(target_os = "linux")]
        if crate::wayland_video::NativeVideo::update(self) {
            tx.instant();
            commit = true;
        }
        if ended && self.video.sleep == crate::video::Sleep::EndOfVideo {
            // The sleep timer was "End of video": stop here, no autoplay.
            self.video.sleep = crate::video::Sleep::Off;
            self.video.pause();
            self.show_toast("Sleep timer: paused at the end of the video", tx);
            return true;
        }
        if ended
            && self.prefs.autoplay
            && !self.video.up_next_cancelled
            && self.top().watch().is_some()
            && self.next_video().is_some()
        {
            let due = now() + 5.0;
            self.video.up_next = Some(due);
            self.video.up_next_text.set("Up next in 5");
            tx.wake_after(Duration::from_secs(1));
            commit = true;
        }
        commit |= self.refresh_controls();
        commit
    }

    fn watch_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let v = &mut self.video;
        let shown = v.shown.clone();
        let paused = !shown.playing();
        match &ev.key {
            Key::Text(t) => match t.as_str() {
                " " | "k" | "K" => {
                    let will_play = paused;
                    v.toggle_pause();
                    v.flash_bezel(will_play);
                }
                "j" | "J" => v.seek_by(-10.0),
                "l" | "L" => v.seek_by(10.0),
                "m" | "M" => v.toggle_mute(),
                "c" | "C" => v.toggle_captions(),
                "f" | "F" => self.toggle_fullscreen(tx),
                "t" | "T" => {
                    if self.fullscreen {
                        self.toggle_fullscreen(tx);
                    }
                    self.toggle_theatre();
                }
                "<" => v.step_speed(false),
                ">" => v.step_speed(true),
                "," if paused => v.seek_by(-1.0 / 30.0),
                "." if paused => v.seek_by(1.0 / 30.0),
                d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                    let n = (d.as_bytes()[0] - b'0') as f32;
                    v.seek_frac(n / 10.0);
                }
                _ => return false,
            },
            Key::Left => v.seek_by(-5.0),
            Key::Right => v.seek_by(5.0),
            Key::Up => {
                let vol = v.volume();
                v.set_volume((vol + 5.0).min(100.0));
            }
            Key::Down => {
                let vol = v.volume();
                v.set_volume((vol - 5.0).max(0.0));
            }
            Key::Home => v.seek(0.0),
            Key::End if v.duration > 0.0 => {
                let end = v.duration;
                v.seek((end - 0.5).max(0.0));
            }
            Key::Escape if self.fullscreen => self.toggle_fullscreen(tx),
            Key::Escape if v.menu != Menu::Closed => v.menu = Menu::Closed,
            Key::Escape if v.stats_open => v.stats_open = false,
            _ => return false,
        }
        true
    }

    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
        if is_quit(ev) {
            tx.quit();
            return true;
        }
        if ev.is_command('w') {
            tx.close_window();
            return true;
        }
        if ev.mods.alt && ev.key == Key::Left {
            self.back(tx);
            return true;
        }
        if ev.mods.alt && ev.key == Key::Right {
            self.go_forward(tx);
            return true;
        }
        let in_field = tx.focused().is_some_and(|f| f.contains("searchbox"));
        if ev.key == Key::Escape && self.close_popups() {
            tx.instant();
            return true;
        }
        if in_field || ev.mods.cmd || ev.mods.alt {
            return false;
        }
        if ev.key == Key::Escape && self.video.stats_open {
            self.video.stats_open = false;
            return true;
        }
        if matches!(&ev.key, Key::Text(c) if c == "?" || ev.mods.shift && c == "/") {
            self.shortcuts = !self.shortcuts;
            tx.instant();
            return true;
        }
        if self.shortcuts {
            return false;
        }
        if matches!(&ev.key, Key::Text(c) if c == "/") {
            self.search_open = true;
            tx.focus("input", false);
            return true;
        }
        // Any key stops a fling (InputRouterImpl::SendKeyboardEvent).
        self.top_mut().scroll.anim.cancel_fling(t);
        if self.top().watch().is_some() && self.watch_key(ev, tx) {
            self.refresh_controls();
            return true;
        }
        let r = self.scroll_key(&ev.key, ev.mods.shift, t);
        if r {
            let now = self.frame_now;
            self.top_mut().scroll.present(now);
            tx.live_only();
            tx.wake_after(Duration::ZERO);
        }
        r
    }

    // ----- small actions --------------------------------------------------

    pub fn show_toast(&mut self, text: &str, tx: &mut Tx) {
        self.toast = Some((text.to_string(), now() + 2.5));
        tx.wake_after(Duration::from_millis(2600));
    }

    pub fn share(&mut self, id: &str, tx: &mut Tx) {
        tx.copy(format!("https://www.youtube.com/watch?v={id}"));
        self.show_toast("Link copied to clipboard", tx);
    }

    pub fn hide_recommendation(&mut self, v: &Video, channel: bool, tx: &mut Tx) {
        if channel {
            if let Some(id) = &v.channel_id {
                library().set_channel_hidden(id, true);
            }
        } else {
            library().set_video_hidden(&v.id, true);
        }
        self.card_menu = None;
        for page in std::iter::once(&mut self.root)
            .chain(&mut self.stack)
            .chain(&mut self.forward)
        {
            match &mut page.kind {
                Kind::Home { .. } | Kind::Watch(_) => {
                    page.feed.items.retain(|v| !library().is_hidden(v));
                }
                Kind::Search(search) => {
                    search.entries.retain(|entry| match entry {
                        nt_core::SearchResult::Video(v) => !library().is_hidden(v),
                        _ => true,
                    });
                    search.trans.items.retain(|v| !library().is_hidden(v));
                    page.feed.items.retain(|v| !library().is_hidden(v));
                }
                _ => continue,
            }
            page.touch();
        }
        self.show_toast(
            if channel {
                "Channel hidden"
            } else {
                "Recommendation hidden"
            },
            tx,
        );
    }

    pub fn toggle_later(&mut self, v: &Video, tx: &mut Tx) {
        let saved = library().toggle_watch_later(v);
        self.show_toast(
            if saved {
                "Saved to Watch Later"
            } else {
                "Removed from Watch Later"
            },
            tx,
        );
        if matches!(self.top().kind, Kind::Later) {
            self.refresh_library_page();
        }
        self.top().touch();
    }

    // ----- autopilot ----------------------------------------------------

    pub fn drain_autopilot(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        while let Some(cmd) = autopilot::try_next() {
            let t = now();
            // Scrolling commands go through the real input paths and only move live values.
            changed |= match cmd {
                Command::Scroll { ticks } => {
                    self.wheel_ticks(ticks, t);
                    false
                }
                Command::ScrollPixels { px } => {
                    self.wheel_pixels(px, t);
                    false
                }
                Command::Fling { velocity } => {
                    self.top_mut().scroll.anim.fling(velocity, t);
                    false
                }
                Command::ScrollToTop => {
                    self.top_mut().scroll.anim.jump_to(0.0);
                    false
                }
                Command::Home { topic } => {
                    let i = topic
                        .and_then(|id| nt_core::TOPICS.iter().position(|x| x.id == id))
                        .unwrap_or(0);
                    self.root(Kind::Home { topic: i }, tx);
                    true
                }
                Command::Search { query } => {
                    self.search.replace_text(query);
                    self.submit_search(tx);
                    true
                }
                Command::OpenVideo { index } => {
                    if let Some(v) = self.top().feed.items.get(index).cloned() {
                        self.open_video(v, tx);
                    }
                    true
                }
                Command::OpenChannel => {
                    let id = self.top().watch().map(|w| w.channel().id);
                    if let Some(id) = id {
                        self.open_channel(id, tx);
                    }
                    true
                }
                Command::Back => {
                    self.back(tx);
                    true
                }
                Command::TogglePause => {
                    self.video.toggle_pause();
                    self.refresh_controls();
                    true
                }
                Command::LoadMoreComments => {
                    self.load_comments();
                    true
                }
                Command::Quit => {
                    tx.quit();
                    true
                }
            };
        }
        tx.wake_after(Duration::ZERO);
        changed
    }

    // ----- frame ----------------------------------------------------------

    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        self.frame_now = now();
        let now = self.frame_now;
        let mut commit = false;
        if self.first_frame {
            self.first_frame = false;
            mark_next(marks::FIRST_FRAME);
        }
        if let Some(due) = self.sugg_due
            && due <= now + 0.002
        {
            self.sugg_due = None;
            self.fetch_suggestions();
        }
        // The field lost focus: close its dropdown (after a moment, so a click on a row lands).
        if self.sugg_open && !self.search.focused {
            match self.blur_due {
                None => {
                    self.blur_due = Some(now + 0.25);
                    tx.wake_after(Duration::from_millis(260));
                }
                Some(due) if due <= now => {
                    self.blur_due = None;
                    self.close_suggestions();
                    commit = true;
                }
                Some(_) => {}
            }
        } else {
            self.blur_due = None;
        }
        if let Some((_, due)) = &self.toast
            && *due <= now
        {
            self.toast = None;
            commit = true;
        }
        let g = self.geo();
        let scale = self.scale.get();
        let page = self.top_mut();
        let (height, list_end, comments_top, related_end, results_end, playlist_end, player) = {
            let plan = page.plan(g);
            (
                plan.height,
                plan.list_end,
                plan.comments_top,
                plan.related_end,
                plan.results_end,
                plan.playlist_end,
                plan.player,
            )
        };
        page.scroll.set_scale(scale);
        page.scroll.set_geometry(g.vh, height);
        if page.scroll.present(now) | page.scroll.take_band_dirty() {
            commit = true;
        }
        if page.scroll.needs_frame() {
            tx.wake_after(Duration::ZERO);
        }
        if let Some(d) = page.scroll.bar_deadline() {
            tx.wake_after(Duration::from_secs_f64((d - now).max(0.0)));
        }
        // Infinite scroll: from where the motion is going, so a fling starts the fetch early.
        let target = page.scroll.anim.target() as f32;
        let near = |end: f32| target + g.vh + 1.5 * g.vh >= end;
        let is_watch = page.watch().is_some();
        let trans_more = match &page.kind {
            Kind::Search(sr) => {
                page.feed.next.is_none() && sr.trans.next.is_some() && sr.trans.more == Load::Idle
            }
            _ => false,
        };
        if !is_watch {
            let playlist_cards = page.playlist_cards_more == Load::Idle
                && page.channel_tab == Some(nt_core::ChannelTab::Playlists)
                && matches!(&page.kind, Kind::Channel { info: Some(info), .. } if info.playlists.next.is_some());
            if playlist_cards && near(list_end) {
                self.load_playlist_cards();
            } else if (page.feed.next.is_some() || trans_more)
                && page.feed.more == Load::Idle
                && page.feed.status == Load::Idle
                && near(if results_end > 0.0 && page.feed.next.is_some() {
                    results_end
                } else {
                    list_end
                })
            {
                self.load_more();
            }
        } else {
            let more_related =
                page.feed.next.is_some() && page.feed.more == Load::Idle && near(related_end);
            let more_playlist = page.watch().is_some_and(|watch| {
                watch.playlist_more == Load::Idle
                    && near(playlist_end)
                    && watch
                        .playlist
                        .as_ref()
                        .is_some_and(|panel| panel.next.is_some())
            });
            if let Some(w) = page.watch() {
                let want = w.comments_state == Load::Idle
                    && if !w.comments_started {
                        w.info.as_ref().is_some_and(|info| info.comments.is_some())
                            && near(comments_top)
                    } else {
                        w.comments_next.is_some()
                            && w.comments_state == Load::Idle
                            && near(list_end)
                    };
                if want {
                    self.load_comments();
                }
            }
            if more_related {
                self.load_more();
            }
            if more_playlist {
                self.load_playlist_more();
            }
            let (bw, bh) = if self.fullscreen {
                (self.win_w, self.win_h)
            } else {
                (player[2], player[3])
            };
            commit |= self.video.set_box(bw, bh, scale);
            if self.video.place_captions(bw, bh, now) {
                tx.wake_after(Duration::from_millis(16));
            }
            commit |= self.test_hooks(tx);
            let next = self
                .prefs
                .autoplay
                .then(|| self.next_video())
                .flatten()
                .map(|v| v.id);
            self.video.prepare_next(next.as_deref());
            commit |= self.watch_timers(tx);
        }
        #[cfg(target_os = "linux")]
        if crate::wayland_video::NativeVideo::update(self) {
            // Rune's element-level instant_changes still springs fill colors.
            // Expose the already-presenting child in the same frame as the state change.
            tx.instant();
            commit = true;
        }
        if self.card_menu.is_some() || self.shortcuts || self.guide_over || self.queue_open {
            commit |= self.preview.cancel();
        } else {
            commit |= self.preview.poll(now, tx);
        }
        crate::probe::tick(self, tx);
        self.mem_trace(tx);
        commit
    }

    /// Test-only switches for input-free checks (captions): `NT_TEST_CC=1` turns captions on
    /// once a video's tracks are known, `NT_TEST_FULLSCREEN=1` enters fullscreen when it is
    /// ready, `NT_TEST_CYCLE=controls` holds the controls shown then hidden, 3.2 s each.
    fn test_hooks(&mut self, tx: &mut Tx) -> bool {
        let mut commit = false;
        let v = &self.video;
        if test_flag("NT_TEST_CC")
            && v.shown.status == nt_core::Status::Ready
            && !v.captions.is_empty()
            && v.shown.cc_sel.is_none()
            && !self.test_cc_done
        {
            self.test_cc_done = true;
            self.video.toggle_captions();
        }
        if test_flag("NT_TEST_FULLSCREEN")
            && self.video.shown.status == nt_core::Status::Ready
            && !self.fullscreen
            && !self.test_fullscreen_done
        {
            self.test_fullscreen_done = true;
            self.toggle_fullscreen(tx);
            commit = true;
        }
        static CYCLE: OnceLock<bool> = OnceLock::new();
        if *CYCLE.get_or_init(|| std::env::var("NT_TEST_CYCLE").is_ok_and(|c| c == "controls"))
            && self.video.shown.status == nt_core::Status::Ready
        {
            let now = now();
            if now >= self.test_cycle_due {
                self.test_cycle_due = now + 3.2;
                self.video.test_controls = !self.video.test_controls;
                eprintln!(
                    "NT_TEST controls {} t={now:.2}",
                    if self.video.test_controls {
                        "shown"
                    } else {
                        "hidden"
                    }
                );
                commit |= self.refresh_controls();
            }
            tx.wake_after(Duration::from_secs_f64(
                (self.test_cycle_due - now).max(0.02),
            ));
        }
        commit
    }

    /// `NT_MEM_TRACE=1`: every 5 s, what the app holds (for attributing memory growth).
    fn mem_trace(&mut self, tx: &mut Tx) {
        static ON: OnceLock<bool> = OnceLock::new();
        if !*ON.get_or_init(|| std::env::var_os("NT_MEM_TRACE").is_some()) {
            return;
        }
        let now = now();
        if now < self.mem_trace_due {
            return;
        }
        self.mem_trace_due = now + 5.0;
        tx.wake_after(Duration::from_secs(5));
        let (n, est, held) = self.images.census();
        let items: usize = std::iter::once(&self.root)
            .chain(self.stack.iter())
            .chain(self.forward.iter())
            .map(|p| p.feed.items.len() + p.watch().map_or(0, |w| w.comments.len()))
            .sum();
        eprintln!(
            "MEMTRACE t={now:.0} pages={} items={items} images={n} est={}MB held={}MB core_cache={}MB frames={}",
            1 + self.stack.len() + self.forward.len(),
            est >> 20,
            held >> 20,
            nt_core::images::cached_bytes() >> 20,
            self.watch_frames.len(),
        );
    }

    /// Controls auto-hide and the Up next countdown.
    fn watch_timers(&mut self, tx: &mut Tx) -> bool {
        let now = now();
        let mut commit = false;
        if let Some(due) = self.video.hide_due
            && due <= now + 0.01
        {
            self.video.hide_due = None;
            commit |= self.refresh_controls();
            if self.video.controls && self.video.pointer_in && self.video.shown.playing() {
                let next = self.video.last_move + crate::video::CONTROLS_HIDE;
                self.video.hide_due = Some(next);
                tx.wake_after(Duration::from_secs_f64((next - now).max(0.05)));
            }
        }
        if let crate::video::Sleep::At(due, _) = self.video.sleep {
            if due <= now + 0.01 {
                self.video.sleep = crate::video::Sleep::Off;
                self.video.pause();
                self.show_toast("Sleep timer: video paused", tx);
                commit = true;
            } else {
                tx.wake_after(Duration::from_secs_f64(due - now));
            }
        }
        if let Some(due) = self.video.up_next {
            let left = due - now;
            if left <= 0.0 {
                self.video.up_next = None;
                self.play_next(tx);
                return true;
            }
            self.video
                .up_next_text
                .set(format!("Up next in {}", left.ceil() as u32));
            tx.wake_after(Duration::from_secs_f64((left - left.floor()).max(0.05)));
        }
        commit
    }
}

/// Ctrl+Q, and Cmd+Q on macOS (Rune reports either as `Command('q')`).
fn is_quit(ev: &KeyEvent) -> bool {
    ev.is_command('q')
        || (ev.mods.ctrl && matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case("q")))
}

fn feed_job(id: u64, req: u64, job: Job<nt_core::Page<Video>>, mark: &'static str) {
    on_result(job, move |s, r, _| {
        let top = s.is_top(id);
        let Some(p) = s.page_for(id, req) else { return };
        match r {
            Ok(page) => {
                p.feed.set(page.items, page.next);
                if top {
                    mark_next(mark);
                }
            }
            Err(e) => p.feed.status = Load::Error(explain(&e)),
        }
        p.touch();
    });
}

/// The touchpad multiplier on top of Chrome's ×12 (`NT_TOUCHPAD_FACTOR`, default 1).
fn touchpad_factor() -> f64 {
    static F: OnceLock<f64> = OnceLock::new();
    *F.get_or_init(|| {
        std::env::var("NT_TOUCHPAD_FACTOR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0)
    })
}

/// A test-only switch (`NAME=1`), read once.
fn test_flag(name: &str) -> bool {
    static FLAGS: OnceLock<Vec<String>> = OnceLock::new();
    FLAGS
        .get_or_init(|| {
            ["NT_TEST_CC", "NT_TEST_FULLSCREEN"]
                .iter()
                .filter(|n| std::env::var(n).is_ok_and(|v| v == "1"))
                .map(|n| n.to_string())
                .collect()
        })
        .iter()
        .any(|n| n == name)
}
