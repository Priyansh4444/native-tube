//! App state, navigation, data loading, input routing and the per-frame `poll`.

use crate::images::Images;
use crate::pages::*;
use crate::scroller::{at, now};
use crate::video::VideoState;
use nt_core::autopilot::{self, Command};
use nt_core::metrics::marks;
use nt_core::{Job, Video, client, library};
use rune::{AppHandle, GesturePhase, Key, KeyEvent, TextEdit, Tx, WheelEvent};
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

/// A metrics mark for the next drawn frame (the one that first shows the content).
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

pub const SUGGEST_DEBOUNCE: f64 = 0.15;

pub struct App {
    pub pages: Vec<Page>,
    pub search: TextEdit,
    pub sugg: Vec<String>,
    pub sugg_open: bool,
    pub sugg_sel: Option<usize>,
    sugg_due: Option<f64>,
    sugg_seq: u64,
    /// The scroll viewport's size, from layout.
    pub vw: f32,
    pub vh: f32,
    pub scale: Cell<f32>,
    pub frame_now: f64,
    pub images: Images,
    pub video: VideoState,
    pub player_hover: bool,
    first_frame: bool,
}

pub fn init() -> App {
    let mut a = App {
        pages: Vec::new(),
        search: TextEdit::new(""),
        sugg: Vec::new(),
        sugg_open: false,
        sugg_sel: None,
        sugg_due: None,
        sugg_seq: 0,
        vw: 1208.0,
        vh: 744.0,
        scale: Cell::new(1.0),
        frame_now: 0.0,
        images: Images::default(),
        video: VideoState::default(),
        player_hover: false,
        first_frame: true,
    };
    a.root(Kind::Home { topic: 0 });
    a
}

impl App {
    pub fn top(&self) -> &Page {
        &self.pages[self.pages.len() - 1]
    }

    pub fn top_mut(&mut self) -> &mut Page {
        let n = self.pages.len() - 1;
        &mut self.pages[n]
    }

    fn page_mut(&mut self, id: u64) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    fn is_top(&self, id: u64) -> bool {
        self.pages.last().is_some_and(|p| p.id == id)
    }

    // ----- navigation ---------------------------------------------------

    /// Leaving the top page: a watch page stops playback and remembers where it was.
    fn leave_top(&mut self) {
        if self.pages.last().is_some_and(|p| p.watch().is_some()) {
            let pos = self.video.stop();
            if let Some(w) = self.top_mut().watch_mut() {
                w.resume_at = pos;
            }
        }
        self.player_hover = false;
    }

    pub fn push(&mut self, kind: Kind) {
        self.leave_top();
        self.close_suggestions();
        self.pages.push(Page::new(kind));
        self.load_top();
    }

    pub fn root(&mut self, kind: Kind) {
        if !self.pages.is_empty() {
            self.leave_top();
        }
        self.close_suggestions();
        self.pages.clear();
        self.pages.push(Page::new(kind));
        self.load_top();
    }

    pub fn back(&mut self) {
        if self.pages.len() < 2 {
            return;
        }
        self.leave_top();
        self.close_suggestions();
        self.pages.pop();
        // Back restores the page as it was: same data and offset, no refetch. A watch page
        // picks its video up where it stopped.
        if let Some((id, at)) = self.top().watch().map(|w| (w.id.clone(), w.resume_at)) {
            self.video.play(&id, at);
        }
        if let Kind::Subs { .. } | Kind::History | Kind::Later = self.top().kind {
            // Library pages show the library as it is now.
            self.refresh_library_page();
        }
    }

    pub fn open_video(&mut self, v: Video) {
        let w = Watch {
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
            resume_at: None,
            comment_h: Default::default(),
        };
        self.push(Kind::Watch(Box::new(w)));
    }

    pub fn open_channel(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        self.push(Kind::Channel { id, info: None });
    }

    pub fn submit_search(&mut self, tx: &mut Tx) {
        let q = self.search.text.trim().to_string();
        self.close_suggestions();
        if q.is_empty() {
            return;
        }
        tx.blur();
        self.push(Kind::Search { query: q });
    }

    // ----- loading ------------------------------------------------------

    /// Starts the top page's first load.
    pub fn load_top(&mut self) {
        let page = self.top_mut();
        let id = page.id;
        page.feed.status = Load::Loading;
        page.touch();
        match &page.kind {
            Kind::Home { topic } => {
                let topic = nt_core::TOPICS.get(*topic).map_or("all", |t| t.id);
                self.feed_job(id, client().home(topic), marks::HOME_LOADED);
            }
            Kind::Search { query } => {
                let job = client().search(query);
                self.feed_job(id, job, marks::SEARCH_LOADED);
            }
            Kind::Channel { id: cid, .. } => {
                on_result(client().channel(cid), move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            p.feed.items = info.videos.items.clone();
                            p.feed.next = info.videos.next.clone();
                            p.feed.status = Load::Idle;
                            if let Kind::Channel { info: slot, .. } = &mut p.kind {
                                *slot = Some(info);
                            }
                            if top {
                                mark_next(marks::CHANNEL_LOADED);
                            }
                        }
                        Err(e) => p.feed.status = Load::Error(e.to_string()),
                    }
                    p.touch();
                });
            }
            Kind::Subs { .. } => {
                let subs = library().subscriptions();
                if let Kind::Subs { channels } = &mut page.kind {
                    *channels = subs.clone();
                }
                if subs.is_empty() {
                    page.feed.status = Load::Idle;
                } else {
                    on_result(client().subscription_feed(subs), move |s, r, _| {
                        let Some(p) = s.page_mut(id) else { return };
                        match r {
                            Ok(items) => {
                                p.feed.items = items;
                                p.feed.status = Load::Idle;
                            }
                            Err(e) => p.feed.status = Load::Error(e.to_string()),
                        }
                        p.touch();
                    });
                }
            }
            Kind::History | Kind::Later => self.refresh_library_page(),
            Kind::Watch(w) => {
                let vid = w.id.clone();
                self.video.play(&vid, None);
                on_result(client().watch(&vid), move |s, r, _| {
                    let top = s.is_top(id);
                    let Some(p) = s.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            library().record_watch(&info.as_video());
                            p.feed.items = info.related.items.clone();
                            p.feed.next = info.related.next.clone();
                            p.feed.status = Load::Idle;
                            if let Some(w) = p.watch_mut() {
                                w.info = Some(info);
                                w.error = None;
                            }
                            if top {
                                mark_next(marks::WATCH_LOADED);
                            }
                        }
                        Err(e) => {
                            p.feed.status = Load::Idle;
                            if let Some(w) = p.watch_mut() {
                                w.error = Some(e.to_string());
                            }
                        }
                    }
                    p.touch();
                });
            }
        }
    }

    fn feed_job(&mut self, id: u64, job: Job<nt_core::Page<Video>>, mark: &'static str) {
        on_result(job, move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_mut(id) else { return };
            match r {
                Ok(page) => {
                    p.feed.items = page.items;
                    p.feed.next = page.next;
                    p.feed.status = Load::Idle;
                    if top {
                        mark_next(mark);
                    }
                }
                Err(e) => p.feed.status = Load::Error(e.to_string()),
            }
            p.touch();
        });
    }

    /// History and Watch Later show the library (newest first).
    pub fn refresh_library_page(&mut self) {
        let p = self.top_mut();
        let items = match p.kind {
            Kind::History => library().history(),
            Kind::Later => library().watch_later(),
            Kind::Subs { .. } => {
                let now = library().subscriptions();
                let same = matches!(&p.kind, Kind::Subs { channels } if *channels == now);
                if !same {
                    self.load_top();
                }
                return;
            }
            _ => return,
        };
        p.feed.items = items;
        p.feed.status = Load::Idle;
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
        let id = page.id;
        page.feed.more = Load::Loading;
        page.touch();
        on_result(client().more_videos(&next), move |s, r, _| {
            let top = s.is_top(id);
            let Some(p) = s.page_mut(id) else { return };
            match r {
                Ok(more) => {
                    let had = p.feed.items.len();
                    p.feed.items.extend(more.items);
                    p.feed.next = more.next;
                    p.feed.more = Load::Idle;
                    if top && p.feed.items.len() > had {
                        mark_next(marks::MORE_LOADED);
                    }
                }
                Err(e) => p.feed.more = Load::Error(e.to_string()),
            }
            p.touch();
        });
    }

    /// The next page of comments on the watch page (the first if none are loaded).
    pub fn load_comments(&mut self) {
        let page = self.top_mut();
        let id = page.id;
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
            let Some(p) = s.page_mut(id) else { return };
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
                    Err(e) => w.comments_state = Load::Error(e.to_string()),
                }
            }
            p.touch();
        });
    }

    pub fn retry(&mut self) {
        let page = self.top_mut();
        if page.feed.status != Load::Idle || page.watch().is_some_and(|w| w.error.is_some()) {
            if let Some(w) = page.watch_mut() {
                w.error = None;
            }
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
            *t = topic;
            page.feed = Feed::default();
            page.scroll.anim.jump_to(0.0);
            self.load_top();
        }
    }

    // ----- suggestions --------------------------------------------------

    pub fn search_changed(&mut self, tx: &mut Tx) {
        let due = now() + SUGGEST_DEBOUNCE;
        self.sugg_due = Some(due);
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
            // A stale answer (an older keystroke's) is dropped.
            if s.sugg_seq != seq {
                return;
            }
            let mut items = r.unwrap_or_default();
            items.truncate(10);
            s.sugg_open = !items.is_empty();
            s.sugg = items;
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
            Key::Escape if self.sugg_open => {
                self.close_suggestions();
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

    pub fn on_wheel(&mut self, e: WheelEvent, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
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

    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let t = at(tx.input_time());
        if ev.mods.alt && ev.key == Key::Left {
            self.back();
            return true;
        }
        let in_field = tx.focused().is_some_and(|f| f.contains("search"));
        if in_field {
            return false;
        }
        if ev.mods.cmd || ev.mods.alt {
            return false;
        }
        // Any key stops a fling (InputRouterImpl::SendKeyboardEvent).
        self.top_mut().scroll.anim.cancel_fling(t);
        if self.top().watch().is_some() && self.video.key(&ev.key) {
            tx.wake_after(Duration::ZERO);
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
                    self.root(Kind::Home { topic: i });
                    true
                }
                Command::Search { query } => {
                    self.search.replace_text(query);
                    self.submit_search(tx);
                    true
                }
                Command::OpenVideo { index } => {
                    let v = self.top().feed.items.get(index).cloned();
                    if let Some(v) = v {
                        self.open_video(v);
                    }
                    true
                }
                Command::OpenChannel => {
                    let id = self.top().watch().and_then(|w| {
                        w.info
                            .as_ref()
                            .map(|i| i.channel.id.clone())
                            .or_else(|| w.video.channel_id.clone())
                    });
                    if let Some(id) = id {
                        self.open_channel(id);
                    }
                    true
                }
                Command::Back => {
                    self.back();
                    true
                }
                Command::TogglePause => {
                    self.video.toggle_pause();
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
        let (vw, vh) = (self.vw, self.vh);
        let wh = vh + crate::theme::TOP_BAR;
        let page = self.top_mut();
        let (height, list_end, comments_top, player) = {
            let plan = page.plan(vw, wh);
            (plan.height, plan.list_end, plan.comments_top, plan.player)
        };
        page.scroll.set_geometry(vh, height);
        if page.scroll.present(now) {
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
        let near_end = |end: f32| target + vh + 1.5 * vh >= end;
        if page.watch().is_none() {
            if page.feed.next.is_some()
                && page.feed.more == Load::Idle
                && page.feed.status == Load::Idle
                && near_end(list_end)
            {
                self.load_more();
            }
        } else if let Some(w) = page.watch() {
            let want = if !w.comments_started {
                w.info.is_some() && target + vh + 1.5 * vh >= comments_top
            } else {
                w.comments_next.is_some() && w.comments_state == Load::Idle && near_end(list_end)
            };
            if want {
                self.load_comments();
            }
            self.video.set_box(player, self.scale.get());
        }
        commit
    }
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
