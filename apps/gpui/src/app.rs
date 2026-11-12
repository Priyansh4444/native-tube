//! The root view: navigation stack, data loading, input routing and the frame hook.

use crate::images::ImageCache;
use crate::pages::*;
use crate::theme::*;
use crate::video::VideoState;
use gpui::prelude::*;
use gpui::Focusable as _;
use gpui::{
    App, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NavigationDirection, ScrollDelta, ScrollEvent, Subscription, TouchPhase, Window, canvas,
    div, px,
};
use gpui_component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::slider::{SliderEvent, SliderState};
use nt_core::autopilot::Command;
use nt_core::metrics::{self, marks};
use nt_core::{Job, TOPICS, Video, client, library};
use std::time::Duration;

/// Seconds on the clock shared by input events, frames and the scroll animators.
pub fn now() -> f64 {
    metrics::now()
}

#[derive(Default)]
pub struct Suggest {
    pub items: Vec<String>,
    pub selected: Option<usize>,
    pub open: bool,
    seq: u64,
}

#[derive(Default)]
pub struct ScrollbarUi {
    pub last_activity: f64,
    /// (pointer y at press, offset at press)
    pub drag: Option<(f32, f64)>,
    hide_pending: bool,
    last_offset: f64,
}

pub struct NtApp {
    pub stack: Vec<Page>,
    next_id: PageId,
    pub search: Entity<InputState>,
    pub sugg: Suggest,
    pub focus: FocusHandle,
    pub images: ImageCache,
    pub video: VideoState,
    pub volume: Entity<SliderState>,
    pub sb: ScrollbarUi,
    /// The last scroll input was a precise (touchpad) delta; only then can a finger lift fling.
    last_precise: bool,
    /// Seek-bar drag: the fraction under the pointer.
    pub seek_drag: Option<f32>,
    /// Seek bar position in window coordinates (x, width), from the last frame.
    pub seek_bar: Option<(f32, f32)>,
    pub geo: Geo,
    pub offset: f64,
    pending_marks: Vec<&'static str>,
    first_frame: bool,
    _subs: Vec<Subscription>,
}

impl NtApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let volume = cx.new(|_| {
            SliderState::new().min(0.0).max(100.0).step(1.0).default_value(library().volume() as f32)
        });
        let subs = vec![
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe_in(&volume, window, |this: &mut Self, _, ev: &SliderEvent, _, cx| {
                let v = match ev {
                    SliderEvent::Change(v) | SliderEvent::Release(v) => v.start(),
                };
                if let Some(p) = this.video.ensure(cx) {
                    p.set_volume(v as f64);
                    if v > 0.0 && !mute_locked() {
                        p.set_muted(false);
                    }
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut app = NtApp {
            stack: Vec::new(),
            next_id: 1,
            search,
            sugg: Suggest::default(),
            focus,
            images: ImageCache::default(),
            video: VideoState::default(),
            volume,
            sb: ScrollbarUi::default(),
            last_precise: false,
            seek_drag: None,
            seek_bar: None,
            geo: Geo { win_w: 1280.0, win_h: 800.0, scale: 1.0 },
            offset: 0.0,
            pending_marks: vec![marks::FIRST_FRAME],
            first_frame: true,
            _subs: subs,
        };
        app.replace_root(Root::Home, cx);
        app.spawn_autopilot(window, cx);
        app
    }

    fn spawn_autopilot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !nt_core::autopilot::enabled() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            while let Some(cmd) = nt_core::autopilot::next_command().await {
                if this.update_in(cx, |app, window, cx| app.run_command(cmd, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // ---- pages ---------------------------------------------------------------------------

    pub fn top(&self) -> Option<&Page> {
        self.stack.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut Page> {
        self.stack.last_mut()
    }

    fn page_mut(&mut self, id: PageId) -> Option<&mut Page> {
        self.stack.iter_mut().find(|p| p.id == id)
    }

    fn is_top(&self, id: PageId) -> bool {
        self.top().is_some_and(|p| p.id == id)
    }

    fn mark_if_top(&mut self, id: PageId, mark: &'static str) {
        if self.is_top(id) {
            self.pending_marks.push(mark);
        }
    }

    fn new_page(&mut self, kind: Kind) -> Page {
        let id = self.next_id;
        self.next_id += 1;
        Page::new(id, kind)
    }

    /// Leaving the watch page on top (another page goes over it, or it's dropped):
    /// stop playback and remember where it was.
    fn leave_top_watch(&mut self) {
        let pos = self.video.state.position;
        if let Some(w) = self.top_mut().and_then(|p| p.watch_mut()) {
            w.resume_at = Some(pos).filter(|p| *p > 1.0);
            self.video.stop();
        }
    }

    pub fn push(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.leave_top_watch();
        let page = self.new_page(kind);
        let id = page.id;
        self.stack.push(page);
        self.sb.last_activity = 0.0;
        self.start_load(id, cx);
        cx.notify();
    }

    pub fn replace_root(&mut self, root: Root, cx: &mut Context<Self>) {
        self.leave_top_watch();
        self.stack.clear();
        let kind = match root {
            Root::Home => Kind::Home { topic: 0 },
            Root::Subscriptions => Kind::Subscriptions { channels: Vec::new() },
            Root::History => Kind::History,
            Root::WatchLater => Kind::WatchLater,
        };
        self.push(kind, cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.stack.len() <= 1 {
            return;
        }
        self.leave_top_watch();
        self.stack.pop();
        // Coming back to a watch page: play the video again from where it was.
        let resume = self.top().and_then(|p| p.watch()).map(|w| w.video.id.clone());
        if let Some(id) = resume {
            self.video.load(&id, cx);
        }
        // Library pages may be stale after a visit elsewhere (e.g. a new Watch Later entry).
        if let Some(page) = self.top_mut()
            && matches!(page.kind, Kind::History | Kind::WatchLater)
        {
            let id = page.id;
            self.start_load(id, cx);
        }
        self.sb.last_activity = 0.0;
        cx.notify();
    }

    pub fn select_topic(&mut self, topic: usize, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        if let Kind::Home { topic: t } = &mut page.kind {
            *t = topic.min(TOPICS.len() - 1);
            page.feed = Feed::default();
            page.scroll.jump_to(0.0);
            let id = page.id;
            self.start_load(id, cx);
            cx.notify();
        }
    }

    pub fn open_video(&mut self, video: Video, cx: &mut Context<Self>) {
        library().record_watch(&video);
        let saved = library().is_saved(&video.id);
        let id = video.id.clone();
        let w = WatchState {
            video,
            info: None,
            comments: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: CommentsState::NotStarted,
            want_comments: false,
            desc_expanded: false,
            subscribed: false,
            saved,
            resume_at: None,
            comment_h: Vec::new(),
            comment_h_width: 0.0,
        };
        self.push(Kind::Watch(Box::new(w)), cx);
        self.video.load(&id, cx);
    }

    pub fn open_channel(&mut self, channel_id: String, cx: &mut Context<Self>) {
        if channel_id.is_empty() {
            return;
        }
        let subscribed = library().is_subscribed(&channel_id);
        self.push(Kind::Channel { id: channel_id, info: None, subscribed }, cx);
    }

    pub fn submit_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.search.update(cx, |s, cx| s.set_value(query.clone(), window, cx));
        self.sugg.open = false;
        self.sugg.selected = None;
        self.sugg.seq += 1;
        window.focus(&self.focus, cx);
        self.push(Kind::Search { query }, cx);
    }

    fn spawn_job<T: Send + 'static>(
        &mut self,
        id: PageId,
        req: u64,
        job: Job<T>,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut NtApp, PageId, nt_core::Result<T>, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if app.stack.iter().any(|p| p.id == id && p.req == req) {
                    apply(app, id, result, cx);
                    if let Some(p) = app.page_mut(id) {
                        p.touch();
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn start_load(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else { return };
        page.status = Load::Loading;
        page.req += 1;
        page.touch();
        let req = page.req;
        match &mut page.kind {
            Kind::Home { topic } => {
                let job = client().home(TOPICS[*topic].id);
                self.spawn_feed(id, req, job, marks::HOME_LOADED, cx);
            }
            Kind::Search { query } => {
                let job = client().search(query);
                self.spawn_feed(id, req, job, marks::SEARCH_LOADED, cx);
            }
            Kind::Channel { id: cid, .. } => {
                let job = client().channel(cid);
                self.spawn_job(id, req, job, cx, |app, id, r, _| {
                    let Some(page) = app.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            page.feed = Feed { items: info.videos.items.clone(), next: info.videos.next.clone(), ..Default::default() };
                            if let Kind::Channel { info: slot, subscribed, .. } = &mut page.kind {
                                *subscribed = library().is_subscribed(&info.id);
                                *slot = Some(info);
                            }
                            page.status = Load::Ready;
                            app.mark_if_top(id, marks::CHANNEL_LOADED);
                        }
                        Err(e) => page.status = Load::Failed(e.to_string()),
                    }
                });
            }
            Kind::Subscriptions { channels } => {
                *channels = library().subscriptions();
                if channels.is_empty() {
                    page.status = Load::Ready;
                    page.feed = Feed::default();
                } else {
                    let job = client().subscription_feed(channels.clone());
                    self.spawn_job(id, req, job, cx, |app, id, r, _| {
                        let Some(page) = app.page_mut(id) else { return };
                        match r {
                            Ok(items) => {
                                page.feed = Feed { items, ..Default::default() };
                                page.status = Load::Ready;
                            }
                            Err(e) => page.status = Load::Failed(e.to_string()),
                        }
                    });
                }
            }
            Kind::History => {
                page.feed = Feed { items: library().history(), ..Default::default() };
                page.status = Load::Ready;
            }
            Kind::WatchLater => {
                page.feed = Feed { items: library().watch_later(), ..Default::default() };
                page.status = Load::Ready;
            }
            Kind::Watch(w) => {
                let job = client().watch(&w.video.id);
                self.spawn_job(id, req, job, cx, |app, id, r, cx| {
                    let Some(page) = app.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            page.feed = Feed { items: info.related.items.clone(), next: info.related.next.clone(), ..Default::default() };
                            page.status = Load::Ready;
                            let mut want = false;
                            if let Some(w) = page.watch_mut() {
                                w.subscribed = library().is_subscribed(&info.channel.id);
                                w.comments_next = info.comments.clone();
                                want = std::mem::take(&mut w.want_comments);
                                w.info = Some(info);
                            }
                            app.mark_if_top(id, marks::WATCH_LOADED);
                            if want {
                                app.load_comments(id, cx);
                            }
                        }
                        Err(e) => page.status = Load::Failed(e.to_string()),
                    }
                });
            }
        }
    }

    fn spawn_feed(&mut self, id: PageId, req: u64, job: Job<nt_core::Page<Video>>, mark: &'static str, cx: &mut Context<Self>) {
        self.spawn_job(id, req, job, cx, move |app, id, r, _| {
            let Some(page) = app.page_mut(id) else { return };
            match r {
                Ok(p) => {
                    page.feed = Feed { items: p.items, next: p.next, ..Default::default() };
                    page.status = Load::Ready;
                    app.mark_if_top(id, mark);
                }
                Err(e) => page.status = Load::Failed(e.to_string()),
            }
        });
    }

    pub fn load_more(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else { return };
        let Some(next) = page.feed.next.clone() else { return };
        if page.feed.loading_more {
            return;
        }
        page.feed.loading_more = true;
        page.feed.more_error = None;
        page.touch();
        let req = page.req;
        let job = client().more_videos(&next);
        self.spawn_job(id, req, job, cx, |app, id, r, _| {
            let Some(page) = app.page_mut(id) else { return };
            page.feed.loading_more = false;
            match r {
                Ok(p) => {
                    page.feed.items.extend(p.items);
                    page.feed.next = p.next;
                    app.mark_if_top(id, marks::MORE_LOADED);
                }
                Err(e) => page.feed.more_error = Some(e.to_string()),
            }
        });
    }

    pub fn load_comments(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else { return };
        let req = page.req;
        let Some(w) = page.watch_mut() else { return };
        if w.comments_state == CommentsState::Loading {
            return;
        }
        if w.info.is_none() {
            w.want_comments = true;
            return;
        }
        let Some(next) = w.comments_next.clone() else {
            // Nothing more to load: the comments that exist are what's on screen.
            if !w.comments.is_empty() {
                self.mark_if_top(id, marks::COMMENTS_LOADED);
                cx.notify();
            }
            return;
        };
        w.comments_state = CommentsState::Loading;
        page.touch();
        let job = client().more_comments(&next);
        self.spawn_job(id, req, job, cx, |app, id, r, _| {
            let Some(w) = app.page_mut(id).and_then(|p| p.watch_mut()) else { return };
            match r {
                Ok(cp) => {
                    w.comments.extend(cp.items);
                    w.comments_next = cp.next;
                    if cp.total.is_some() {
                        w.comments_total = cp.total;
                    }
                    w.comments_state = CommentsState::Idle;
                    app.mark_if_top(id, marks::COMMENTS_LOADED);
                }
                Err(e) => w.comments_state = CommentsState::Failed(e.to_string()),
            }
        });
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        let id = page.id;
        if page.status != Load::Ready {
            self.start_load(id, cx);
        } else if page.feed.more_error.is_some() {
            page.feed.more_error = None;
            self.load_more(id, cx);
        } else if let Some(w) = page.watch_mut()
            && matches!(w.comments_state, CommentsState::Failed(_))
        {
            w.comments_state = CommentsState::Idle;
            self.load_comments(id, cx);
        }
        cx.notify();
    }

    pub fn toggle_subscribe(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        match &mut page.kind {
            Kind::Watch(w) => {
                if let Some(info) = &w.info {
                    w.subscribed = library().toggle_subscription(&info.channel);
                }
            }
            Kind::Channel { info: Some(info), subscribed, .. } => {
                *subscribed = library().toggle_subscription(&info.as_ref());
            }
            _ => {}
        }
        page.touch();
        cx.notify();
    }

    pub fn toggle_watch_later(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.top_mut().and_then(|p| p.watch_mut()) {
            let mut v = w.info.as_ref().map(|i| i.as_video()).unwrap_or_else(|| w.video.clone());
            if v.duration.is_none() {
                v.duration = w.video.duration.clone();
            }
            w.saved = library().toggle_watch_later(&v);
            cx.notify();
        }
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        library().clear_history();
        if let Some(page) = self.top_mut() {
            let id = page.id;
            self.start_load(id, cx);
        }
        cx.notify();
    }

    // ---- player ----------------------------------------------------------------------------

    pub fn on_player_wake(&mut self, cx: &mut Context<Self>) {
        if self.video.poll() {
            // Resume after Back once the video is playing again.
            if self.video.state.status == nt_core::Status::Ready
                && let Some(w) = self.stack.last_mut().and_then(|p| p.watch_mut())
                && let Some(at) = w.resume_at.take()
                && let Some(p) = &self.video.player
            {
                p.seek(at);
            }
            if self.top().is_some_and(|p| p.watch().is_some()) {
                cx.notify();
            }
        }
    }

    pub fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.video.player {
            p.toggle_pause();
        }
        cx.notify();
    }

    pub fn toggle_mute(&mut self) {
        if let Some(p) = &self.video.player {
            p.set_muted(!self.video.state.muted || mute_locked());
        }
    }

    pub fn nudge_volume(&mut self, delta: f64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.video.player {
            let v = (self.video.state.volume + delta).clamp(0.0, 100.0);
            p.set_volume(v);
            p.set_muted(mute_locked());
            self.volume.update(cx, |s, cx| s.set_value(v as f32, window, cx));
        }
    }

    pub fn seek_fraction(&mut self, f: f32) {
        let d = self.video.state.duration;
        if let Some(p) = &self.video.player
            && d > 0.0
        {
            p.seek(f.clamp(0.0, 1.0) as f64 * d);
        }
    }

    fn seek_frac_at(&self, x: f32) -> Option<f32> {
        let (x0, w) = self.seek_bar?;
        Some(((x - x0) / w.max(1.0)).clamp(0.0, 1.0))
    }

    // ---- scrolling -------------------------------------------------------------------------

    fn activity(&mut self, t: f64, cx: &mut Context<Self>) {
        self.sb.last_activity = t;
        cx.notify();
    }

    /// Mouse-wheel notches (the autopilot's `Scroll` lands here too).
    pub fn wheel_ticks(&mut self, ticks: f64, t: f64, cx: &mut Context<Self>) {
        self.last_precise = false;
        if let Some(p) = self.top_mut() {
            p.scroll.wheel_ticks(ticks, t);
        }
        self.activity(t, cx);
    }

    /// Precise touchpad pixels (the autopilot's `ScrollPixels` lands here too).
    pub fn wheel_pixels(&mut self, px: f64, t: f64, cx: &mut Context<Self>) {
        self.last_precise = true;
        if let Some(p) = self.top_mut() {
            p.scroll.wheel_pixels(px, t);
        }
        self.activity(t, cx);
    }

    pub fn on_scroll_event(&mut self, ev: &ScrollEvent, cx: &mut Context<Self>) {
        let t = now();
        if trace_scroll() {
            eprintln!("scroll t={t:.3} {:?} {:?}", ev.delta, ev.touch_phase);
        }
        match ev.delta {
            // GPUI's Wayland backend reports a notch as 3 lines, negated (value120/120 * 3 * -1).
            ScrollDelta::Lines(d) => {
                let ticks = -(d.y as f64) / 3.0;
                if ticks != 0.0 {
                    self.wheel_ticks(ticks, t, cx);
                }
            }
            ScrollDelta::Pixels(d) => {
                if ev.touch_phase == TouchPhase::Ended {
                    // Finger lift (axis_stop, via the patched backend). Chrome only sees
                    // axis_stop after finger frames; Hyprland's virtual pointer reports the
                    // frames themselves as wheel value120, so don't fling off wheel samples.
                    if !std::mem::take(&mut self.last_precise) {
                        return;
                    }
                    if let Some(p) = self.top_mut() {
                        p.scroll.fling_start(t);
                        if trace_scroll() {
                            eprintln!("fling_start: v={:.0} target={:.0}", p.scroll.velocity(), p.scroll.target());
                        }
                    }
                    self.activity(t, cx);
                } else {
                    // The backend scales wl_pointer.axis by -3; Chrome uses x12.
                    let k = if ev.source == gpui::ScrollSource::Touch { -1.0 } else { -4.0 };
                    let px = f64::from(f32::from(d.y)) * k;
                    if px != 0.0 {
                        self.wheel_pixels(px, t, cx);
                    }
                }
            }
        }
    }

    fn search_focused(&self, window: &Window, cx: &App) -> bool {
        self.search.read(cx).focus_handle(cx).is_focused(window)
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focused(window, cx) {
            return;
        }
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        let m = ks.modifiers;
        let t = now();
        if m.alt && key == "left" {
            self.back(cx);
            return;
        }
        // Focus the search box, as on YouTube ("/") and in browsers (Ctrl+L).
        if (key == "/" && !m.control && !m.alt) || (m.control && key == "l") {
            self.search.update(cx, |s, cx| {
                s.focus(window, cx);
                s.select_all(window, cx);
            });
            cx.stop_propagation();
            return;
        }
        if m.control || m.alt || m.platform {
            return;
        }
        if self.top().is_some_and(|p| p.watch().is_some()) && self.video.player.is_some() {
            let handled = match key {
                "space" | "k" => {
                    self.toggle_pause(cx);
                    true
                }
                "j" | "l" | "left" | "right" => {
                    let d = match key {
                        "j" => -10.0,
                        "l" => 10.0,
                        "left" => -5.0,
                        _ => 5.0,
                    };
                    if let Some(p) = &self.video.player {
                        p.seek_by(d);
                    }
                    true
                }
                "m" => {
                    self.toggle_mute();
                    true
                }
                "up" => {
                    self.nudge_volume(5.0, window, cx);
                    true
                }
                "down" => {
                    self.nudge_volume(-5.0, window, cx);
                    true
                }
                _ => false,
            };
            if handled {
                if let Some(p) = self.top_mut() {
                    p.scroll.cancel_fling(t);
                }
                cx.stop_propagation();
                return;
            }
        }
        let vh = self.geo.viewport_h(self.top().is_some_and(|p| p.has_chips())) as f64;
        let Some(page) = self.top_mut() else { return };
        let s = &mut page.scroll;
        let delta = match key {
            "up" => Some(-s.line_step()),
            "down" => Some(s.line_step()),
            "pageup" => Some(-s.page_step(vh)),
            "pagedown" => Some(s.page_step(vh)),
            "space" if m.shift => Some(-s.page_step(vh)),
            "space" => Some(s.page_step(vh)),
            "home" => Some(-s.document_step()),
            "end" => Some(s.document_step()),
            _ => None,
        };
        match delta {
            Some(d) => {
                s.scroll_by(d, t);
                self.activity(t, cx);
                cx.stop_propagation();
            }
            // Any other key stops a fling, as in Chrome.
            None => s.cancel_fling(t),
        }
    }

    // ---- search box ------------------------------------------------------------------------

    fn on_search_event(&mut self, state: &Entity<InputState>, ev: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                self.sugg.seq += 1;
                self.sugg.selected = None;
                let seq = self.sugg.seq;
                if text.trim().is_empty() {
                    self.sugg.items.clear();
                    self.sugg.open = false;
                    cx.notify();
                    return;
                }
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    if this.read_with(cx, |app, _| app.sugg.seq != seq).unwrap_or(true) {
                        return;
                    }
                    let result = client().suggestions(&text).await;
                    let _ = this.update(cx, |app, cx| {
                        // A newer keystroke (or a submit) makes this answer stale.
                        if app.sugg.seq != seq {
                            return;
                        }
                        if trace_scroll() {
                            eprintln!("suggestions: {result:?}");
                        }
                        if let Ok(mut items) = result {
                            items.truncate(10);
                            app.sugg.open = !items.is_empty();
                            app.sugg.items = items;
                            app.sugg.selected = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            InputEvent::PressEnter { .. } => {
                let q = match self.sugg.selected.and_then(|i| self.sugg.items.get(i)) {
                    Some(s) if self.sugg.open => s.clone(),
                    _ => state.read(cx).value().to_string(),
                };
                self.submit_search(q, window, cx);
            }
            // Suggestions reappear on the next keystroke, not on focus: a window regaining
            // activation refocuses the input too, and that shouldn't pop the list open.
            InputEvent::Focus => {}
            // The window losing activation blurs the input too; only close for a real blur.
            InputEvent::Blur if window.is_window_active() => {
                self.sugg.open = false;
                cx.notify();
            }
            InputEvent::Blur => {}
        }
    }

    pub fn sugg_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if !self.sugg.open || self.sugg.items.is_empty() {
            return false;
        }
        let n = self.sugg.items.len();
        self.sugg.selected = Some(match (self.sugg.selected, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        });
        cx.notify();
        true
    }

    // ---- autopilot -------------------------------------------------------------------------

    fn run_command(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        let t = now();
        match cmd {
            Command::Home { topic } => {
                self.replace_root(Root::Home, cx);
                if let Some(topic) = topic.and_then(|id| TOPICS.iter().position(|x| x.id == id))
                    && topic != 0
                {
                    self.select_topic(topic, cx);
                }
            }
            Command::Search { query } => self.submit_search(query, window, cx),
            Command::OpenVideo { index } => {
                if let Some(v) = self.top().and_then(|p| p.feed.items.get(index)).cloned() {
                    self.open_video(v, cx);
                }
            }
            Command::OpenChannel => {
                let id = self.top().and_then(|p| p.watch()).and_then(|w| {
                    w.info.as_ref().map(|i| i.channel.id.clone()).or_else(|| w.video.channel_id.clone())
                });
                if let Some(id) = id {
                    self.open_channel(id, cx);
                }
            }
            Command::Back => self.back(cx),
            Command::Scroll { ticks } => self.wheel_ticks(ticks, t, cx),
            Command::ScrollPixels { px } => self.wheel_pixels(px, t, cx),
            Command::Fling { velocity } => {
                if let Some(p) = self.top_mut() {
                    p.scroll.fling(velocity, t);
                }
                self.activity(t, cx);
            }
            Command::ScrollToTop => {
                if let Some(p) = self.top_mut() {
                    p.scroll.jump_to(0.0);
                }
                self.activity(t, cx);
            }
            Command::TogglePause => self.toggle_pause(cx),
            Command::LoadMoreComments => {
                if let Some(id) = self.top().map(|p| p.id) {
                    self.load_comments(id, cx);
                }
            }
            Command::Quit => {
                self.video.stop();
                self.video.player = None;
                cx.quit();
            }
        }
    }

    // ---- frame ----------------------------------------------------------------------------

    /// Lay out the top page if its geometry or data changed; feed the animator the range.
    fn update_layout(&mut self, window: &mut Window) {
        let geo = self.geo;
        let Some(page) = self.stack.last_mut() else { return };
        let key = LayoutKey { geo, version: page.version };
        if page.layout.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let mut m = WindowMeasure { window };
        let lay = build_layout(page, &geo, &mut m);
        let vh = geo.viewport_h(page.has_chips()) as f64;
        page.scroll.set_viewport(vh);
        page.scroll.set_max_offset(lay.content_h as f64 - vh);
        page.layout = Some((key, lay));
    }

    /// Infinite scroll and the comments trigger, checked against where motion will end.
    fn triggers(&mut self, cx: &mut Context<Self>) {
        let vh = self.geo.viewport_h(self.top().is_some_and(|p| p.has_chips())) as f64;
        let Some(page) = self.stack.last() else { return };
        let Some((_, lay)) = &page.layout else { return };
        let id = page.id;
        let bottom = page.scroll.target() + vh;
        if page.status == Load::Ready
            && page.feed.next.is_some()
            && !page.feed.loading_more
            && page.feed.more_error.is_none()
            && lay.main_end as f64 - bottom < 1.5 * vh
        {
            self.load_more(id, cx);
        }
        let Some(page) = self.stack.last() else { return };
        let Some((_, lay)) = &page.layout else { return };
        if let Some(w) = page.watch()
            && let Some(cy) = lay.comments_y
        {
            let first = w.comments.is_empty() && w.comments_state == CommentsState::NotStarted && (cy as f64) - bottom < vh;
            let more = w.comments_state == CommentsState::Idle
                && w.comments_next.is_some()
                && (lay.primary_end as f64) - bottom < 1.5 * vh;
            if first || more {
                self.load_comments(id, cx);
            }
        }
    }

    fn schedule_scrollbar_hide(&mut self, cx: &mut Context<Self>) {
        if self.sb.hide_pending {
            return;
        }
        self.sb.hide_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1300)).await;
            let _ = this.update(cx, |app, cx| {
                app.sb.hide_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn scrollbar_visible(&self, t: f64) -> bool {
        self.sb.drag.is_some() || t - self.sb.last_activity < 1.0
    }
}

/// Test and benchmark runs set `NT_MUTE=1`; then nothing in the UI may unmute.
fn mute_locked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NT_MUTE").is_ok_and(|v| v != "0"))
}

/// `NT_GPUI_TRACE_SCROLL=1` logs raw scroll events (a debugging aid).
fn trace_scroll() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NT_GPUI_TRACE_SCROLL").is_some())
}

struct WindowMeasure<'a> {
    window: &'a mut Window,
}

impl Measure for WindowMeasure<'_> {
    fn lines(&mut self, text: &str, font_size: f32, weight: u16, width: f32) -> usize {
        if text.is_empty() {
            return 0;
        }
        let mut font = self.window.text_style().font();
        font.weight = gpui::FontWeight(weight as f32);
        let run = gpui::TextRun { len: text.len(), font, color: text_color(), background_color: None, underline: None, strikethrough: None };
        match self.window.text_system().shape_text(text.to_string().into(), px(font_size), &[run], Some(px(width)), None) {
            Ok(lines) => lines.iter().map(|l| 1 + l.wrap_boundaries().len()).sum(),
            Err(_) => text.lines().count().max(1),
        }
    }
}

fn text_color() -> gpui::Hsla {
    crate::theme::text()
}

impl Render for NtApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = now();
        self.images.begin_frame();
        let vp = window.viewport_size();
        self.geo = Geo { win_w: f32::from(vp.width), win_h: f32::from(vp.height), scale: window.scale_factor() };

        self.update_layout(window);
        self.triggers(cx);
        self.update_layout(window);

        // Advance the scroll animation to this frame and keep frames coming only while it moves.
        if let Some(page) = self.stack.last_mut() {
            let off = page.scroll.tick(t);
            if page.scroll.needs_frame() {
                window.request_animation_frame();
            }
            let s = self.geo.scale as f64;
            self.offset = (off * s).round() / s;
        }
        if self.offset != self.sb.last_offset {
            self.sb.last_offset = self.offset;
            self.sb.last_activity = t;
        }
        if self.scrollbar_visible(t) && !self.stack.last().is_some_and(|p| p.scroll.needs_frame()) {
            self.schedule_scrollbar_hide(cx);
        }

        // The newest video picture, if the watch page is up.
        if self.top().is_some_and(|p| p.watch().is_some()) && self.video.upload(window) && self.video.started_pending {
            self.video.started_pending = false;
            self.pending_marks.push(marks::PLAYBACK_STARTED);
        }

        let content = crate::views::content(self, t, cx);
        let rail = crate::views::rail(self, cx);
        let top_bar = crate::views::top_bar(self, cx);

        self.images.evict(window);

        // The frame hook: this canvas paints once per drawn frame, right before present.
        let marks_now = if self.first_frame || !self.pending_marks.is_empty() {
            self.first_frame = false;
            std::mem::take(&mut self.pending_marks)
        } else {
            Vec::new()
        };
        let hook = canvas(
            |_, _, _| (),
            move |_, _, _, _| {
                metrics::frame();
                for m in marks_now {
                    metrics::mark(m);
                }
            },
        )
        .absolute()
        .size_0();

        div()
            .id("nt-root")
            .size_full()
            .relative()
            .bg(bg())
            .text_color(text())
            .on_key_down(cx.listener(Self::on_key_down))

            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.back(cx)),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let y = f32::from(ev.position.y);
                if let Some((y0, off0)) = this.sb.drag {
                    let chips = this.top().is_some_and(|p| p.has_chips());
                    let vh = this.geo.viewport_h(chips);
                    if let Some(page) = this.top_mut() {
                        let max = page.scroll.max_offset();
                        let content = max + vh as f64;
                        let thumb = (vh as f64 * vh as f64 / content.max(1.0)).max(32.0);
                        let track = (vh as f64 - thumb).max(1.0);
                        page.scroll.jump_to(off0 + (y - y0) as f64 * max / track);
                    }
                    this.activity(now(), cx);
                }
                if this.seek_drag.is_some()
                    && let Some(f) = this.seek_frac_at(f32::from(ev.position.x))
                {
                    this.seek_drag = Some(f);
                    this.seek_fraction(f);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.sb.drag.take().is_some() || this.seek_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(content)
            .child(rail)
            .child(top_bar)
            .child(hook)
    }
}

pub fn capture_search_keys(this: &mut NtApp, down: Option<bool>, cx: &mut Context<NtApp>) {
    match down {
        Some(d) => {
            if this.sugg_move(d, cx) {
                cx.stop_propagation();
            }
        }
        None => {
            if this.sugg.open {
                this.sugg.open = false;
                cx.notify();
                cx.stop_propagation();
            }
        }
    }
}

pub fn search_key_handlers(div: gpui::Div, cx: &mut Context<NtApp>) -> gpui::Div {
    div.capture_action(cx.listener(|this, _: &MoveUp, _, cx| capture_search_keys(this, Some(false), cx)))
        .capture_action(cx.listener(|this, _: &MoveDown, _, cx| capture_search_keys(this, Some(true), cx)))
        .capture_action(cx.listener(|this, _: &Escape, _, cx| capture_search_keys(this, None, cx)))
}
