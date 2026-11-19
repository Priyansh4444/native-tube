//! App state, navigation, data loading, input routing and the per-frame `poll`.

use crate::images::Images;
use crate::pages::*;
use crate::scroller::{at, now};
use crate::video::{Menu, VideoState};
use nt_core::autopilot::{self, Command};
use nt_core::metrics::marks;
use nt_core::{Job, Video, client, library};
use rune::{AppHandle, GesturePhase, Key, KeyEvent, Live, TextEdit, Tx, WheelEvent};
use std::cell::Cell;
use std::sync::{Mutex, OnceLock};
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
        }
    }

    /// Saved on a thread of its own: never file I/O on the UI thread.
    fn save(self) {
        let text = format!(
            "autoplay={}\ntheatre={}\n",
            self.autoplay as u8, self.theatre as u8
        );
        let _ = std::thread::Builder::new()
            .name("nt-prefs".into())
            .spawn(move || {
                let path = Self::path();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension("tmp");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            });
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
    pub progress: Progress,
    pub prefs: Prefs,
    pub fullscreen: bool,
    pub card_menu: Option<CardMenu>,
    pub toast: Option<(String, f64)>,
    /// The Home chip bar's horizontal offset, its content width and the live translation.
    pub chips_x: f32,
    pub chips_live: Live<[f32; 2]>,
    first_frame: bool,
}

pub fn init() -> App {
    App {
        root: home_page(0),
        stack: Vec::new(),
        forward: Vec::new(),
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
        progress: Progress::default(),
        prefs: Prefs::load(),
        fullscreen: false,
        card_menu: None,
        toast: None,
        chips_x: 0.0,
        chips_live: Live::new([0.0, 0.0]),
        first_frame: true,
    }
    .with_loaded_root()
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

    /// The page with this id, if it is still on the stack and `req` is still its request.
    fn page_for(&mut self, id: u64, req: u64) -> Option<&mut Page> {
        if self.root.id == id {
            return (self.root.req == req).then_some(&mut self.root);
        }
        self.stack
            .iter_mut()
            .find(|p| p.id == id)
            .filter(|p| p.req == req)
    }

    fn is_top(&self, id: u64) -> bool {
        self.top().id == id
    }

    /// The geometry the top page is laid out for.
    pub fn geo(&self) -> Geo {
        Geo {
            win_w: self.win_w,
            win_h: self.win_h,
            vw: self.vw,
            vh: self.vh,
            theatre: self.prefs.theatre,
        }
    }

    // ----- navigation ---------------------------------------------------

    /// Leaving the top page: a watch page stops playback and remembers where it was.
    fn leave_top(&mut self, tx: &mut Tx) {
        if self.top().watch().is_some() {
            if let (Some(f), Some(id)) = (
                self.video.played_fraction(),
                self.top().watch().map(|w| w.id.clone()),
            ) {
                self.progress.set(&id, f);
            }
            let pos = self.video.stop();
            if let Some(w) = self.top_mut().watch_mut() {
                w.resume_at = pos;
            }
            if self.fullscreen {
                self.fullscreen = false;
                tx.toggle_full_screen();
            }
        }
        self.video.pointer_in = false;
        self.card_menu = None;
        self.close_suggestions();
        self.search_open = false;
    }

    /// The top page came back into view (Back, Forward): its video resumes where it stopped;
    /// library pages show the library as it is now.
    fn enter_top(&mut self) {
        if let Some((id, at)) = self.top().watch().map(|w| (w.id.clone(), w.resume_at)) {
            self.video.play(&id, at);
        }
        if let Kind::Subs { .. } | Kind::History | Kind::Later = self.top().kind {
            self.refresh_library_page();
        }
        // A results page brings its query back into the search box.
        if let Kind::Search { query } = &self.top().kind
            && self.search.text != *query
        {
            let q = query.clone();
            self.search.replace_text(q);
            let end = self.search.text.len();
            self.search.set_caret(end);
        }
        self.top().touch();
    }

    pub fn push(&mut self, kind: Kind, tx: &mut Tx) {
        self.leave_top(tx);
        self.forward.clear();
        // Keep the root; drop the oldest pages above it once the stack is deep.
        if self.stack.len() >= MAX_BACK {
            self.stack.remove(0);
        }
        self.stack.push(Page::new(kind));
        self.load_top();
    }

    pub fn root(&mut self, kind: Kind, tx: &mut Tx) {
        self.leave_top(tx);
        self.stack.clear();
        self.forward.clear();
        self.root = Page::new(kind);
        self.load_top();
    }

    pub fn back(&mut self, tx: &mut Tx) {
        if self.stack.is_empty() {
            return;
        }
        self.leave_top(tx);
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
        self.leave_top(tx);
        self.stack.push(p);
        self.enter_top();
    }

    pub fn open_video(&mut self, v: Video, tx: &mut Tx) {
        library().record_watch(&v);
        let id = v.id.clone();
        self.push(Kind::Watch(Box::new(Watch::new(v))), tx);
        self.video.play(&id, None);
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

    pub fn submit_search(&mut self, tx: &mut Tx) {
        let q = self.search.text.trim().to_string();
        self.close_suggestions();
        if q.is_empty() {
            return;
        }
        tx.blur();
        self.search_open = false;
        self.push(Kind::Search { query: q }, tx);
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
                let topic = nt_core::TOPICS.get(*topic).map_or("all", |t| t.id);
                feed_job(id, req, client().home(topic), marks::HOME_LOADED);
            }
            Kind::Search { query } => {
                feed_job(id, req, client().search(query), marks::SEARCH_LOADED);
            }
            Kind::Channel { id: cid, .. } => {
                on_result(client().channel(cid), move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_for(id, req) else { return };
                    match r {
                        Ok(info) => {
                            p.feed
                                .set(info.videos.items.clone(), info.videos.next.clone());
                            if let Kind::Channel { info: slot, .. } = &mut p.kind {
                                *slot = Some(info);
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
                on_result(client().watch(&vid), move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_for(id, req) else { return };
                    match r {
                        Ok(info) => {
                            p.feed
                                .set(info.related.items.clone(), info.related.next.clone());
                            if let Some(w) = p.watch_mut() {
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
        let page = self.top_mut();
        if page.feed.status != Load::Idle
            || page.feed.more == Load::Loading
            || page.watch().is_some()
        {
            return;
        }
        let Some(next) = page.feed.next.clone() else {
            return;
        };
        let (id, req) = (page.id, page.req);
        page.feed.more = Load::Loading;
        page.touch();
        on_result(client().more_videos(&next), move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_for(id, req) else { return };
            match r {
                Ok(more) => {
                    let added = p.feed.append(more.items);
                    p.feed.next = more.next;
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
        let cont = if !w.comments_started {
            w.info.as_ref().and_then(|i| i.comments.clone())
        } else {
            w.comments_next.clone()
        };
        let Some(cont) = cont else { return };
        w.comments_started = true;
        w.comments_state = Load::Loading;
        page.touch();
        on_result(client().more_comments(&cont), move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_for(id, req) else { return };
            if let Some(w) = p.watch_mut() {
                match r {
                    Ok(c) => {
                        w.comments.extend(c.items);
                        w.comments_next = c.next;
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

    /// Retry what failed, and only that.
    pub fn retry(&mut self) {
        let page = self.top_mut();
        if matches!(page.feed.status, Load::Error(_))
            || page.watch().is_some_and(|w| w.error.is_some())
        {
            self.load_top();
        } else if matches!(page.feed.more, Load::Error(_)) {
            page.feed.more = Load::Idle;
            self.load_more();
        } else if page
            .watch()
            .is_some_and(|w| matches!(w.comments_state, Load::Error(_)))
        {
            if let Some(w) = page.watch_mut() {
                w.comments_state = Load::Idle;
            }
            self.load_comments();
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
        let room = (crate::view::chips_width() - (self.vw - 2.0 * self.geo().pad())).max(0.0);
        self.chips_x = (self.chips_x + dx).clamp(0.0, room);
        self.chips_live.set([-self.chips_x, 0.0]);
    }

    pub fn on_wheel(&mut self, e: WheelEvent, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
        self.card_menu = None;
        // Horizontal wheel or Shift+wheel over the Home chip bar scrolls the chips.
        let rail = if self.top().has_rail(self.win_w) {
            crate::theme::RAIL
        } else {
            0.0
        };
        let over_chips = self.top().has_chips()
            && e.y >= crate::theme::TOP_BAR
            && e.y < crate::theme::TOP_BAR + crate::theme::CHIP_BAR
            && e.x >= rail;
        if over_chips && e.dx != 0.0 {
            self.scroll_chips(-e.dx);
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
        tx.live_only();
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
        if !self.refresh_controls() {
            tx.live_only();
        }
    }

    pub fn player_hover(&mut self, on: bool) {
        self.video.pointer_in = on;
        self.video.last_move = now();
        self.refresh_controls();
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
    fn play_next(&mut self, tx: &mut Tx) {
        let next = self.top().feed.items.first().cloned();
        if let Some(v) = next {
            self.open_video(v, tx);
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
        if ended
            && self.prefs.autoplay
            && !self.video.up_next_cancelled
            && self.top().watch().is_some()
            && !self.top().feed.items.is_empty()
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
        if in_field || ev.mods.cmd || ev.mods.alt {
            return false;
        }
        if ev.key == Key::Escape && self.card_menu.is_some() {
            self.card_menu = None;
            return true;
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
        if crate::video::take_first_picture() {
            mark_next(marks::PLAYBACK_STARTED);
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
        let (height, list_end, comments_top, player) = {
            let plan = page.plan(g);
            (plan.height, plan.list_end, plan.comments_top, plan.player)
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
        if !is_watch {
            if page.feed.next.is_some()
                && page.feed.more == Load::Idle
                && page.feed.status == Load::Idle
                && near(list_end)
            {
                self.load_more();
            }
        } else {
            if let Some(w) = page.watch() {
                let want = if !w.comments_started {
                    w.info.is_some() && target + g.vh + 1.5 * g.vh >= comments_top
                } else {
                    w.comments_next.is_some() && w.comments_state == Load::Idle && near(list_end)
                };
                if want {
                    self.load_comments();
                }
            }
            let (bw, bh) = if self.fullscreen {
                (self.win_w, self.win_h)
            } else {
                (player[2], player[3])
            };
            self.video.set_box(bw, bh, scale);
            commit |= self.watch_timers(tx);
        }
        commit
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
