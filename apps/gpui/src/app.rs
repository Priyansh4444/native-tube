//! The root view: navigation, data loading, input routing, the scroll tick and the frame hook.
//!
//! The window is split into views so a frame only redoes what changed:
//! - `TopBar` and `Rail` are cached views: reused unless they are notified.
//! - `PageView` draws the current page (chips, blocks, scrollbar). It is cached too; anything
//!   that changes the page calls `content_changed`, which notifies it.
//! - `PlayerView` is a sibling of the page, positioned over the page's player block. A video
//!   frame notifies only it (and so the cheap root), never the page.

use crate::images::ImageCache;
use crate::pages::*;
use crate::player::{HIDE_CURSOR_KEY, PlayerEvent, PlayerView};
use crate::prefs::Prefs;
use crate::rail::{Navigate, Rail};
use crate::theme::*;
use crate::top_bar::{TopBar, TopBarEvent};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    NavigationDirection, ScrollDelta, ScrollEvent, StyleRefinement, Subscription, TouchPhase,
    WeakEntity, Window, canvas, div, px,
};
use nt_core::autopilot::Command;
use nt_core::metrics::{self, marks};
use nt_core::{Job, TOPICS, Video, client, library};
use std::time::Duration;

gpui::actions!(nativetube, [Quit, CloseWindow]);

const MAX_STACK: usize = 30;

/// Seconds on the clock shared by input events, frames and the scroll animators.
pub fn now() -> f64 {
    metrics::now()
}

#[derive(Default)]
pub struct ScrollbarUi {
    pub last_activity: f64,
    /// (pointer y at press, offset at press)
    pub drag: Option<(f32, f64)>,
    hide_pending: bool,
}

/// The "⋮" menu of a card or row, at window coordinates.
pub struct CardMenu {
    pub video: Video,
    pub x: f32,
    pub y: f32,
}

pub struct PageView {
    pub app: WeakEntity<NtApp>,
}

impl Render for PageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.render_page(window, cx)),
            None => div().into_any_element(),
        }
    }
}

pub struct NtApp {
    pub stack: Vec<Page>,
    /// Pages popped by Back, for Forward.
    forward: Vec<Page>,
    next_id: PageId,
    pub images: ImageCache,
    pub prefs: Prefs,
    pub page_view: Entity<PageView>,
    pub top_bar: Entity<TopBar>,
    pub rail: Entity<Rail>,
    pub player: Entity<PlayerView>,
    pub focus: FocusHandle,
    pub geo: Geo,
    pub offset: f64,
    pub sb: ScrollbarUi,
    pub theatre: bool,
    pub fullscreen: bool,
    frame_pending: bool,
    /// The last scroll input was a precise (touchpad) delta; only then can a finger lift fling.
    last_precise: bool,
    pending_marks: Vec<&'static str>,
    first_frame: bool,
    pub menu: Option<CardMenu>,
    /// A short confirmation at the bottom left ("Link copied"), and when it was shown.
    toast: Option<(&'static str, f64)>,
    /// Horizontal scroll of the chip bar, and how far it can go.
    pub chips_x: f32,
    pub chips_max: f32,
    pub chip_widths: Vec<f32>,
    _subs: Vec<Subscription>,
}

impl NtApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = Prefs::load();
        let me = cx.entity().downgrade();
        let page_view = cx.new(|_| PageView { app: me });
        let top_bar = cx.new(|cx| TopBar::new(window, cx));
        let rail = cx.new(|_| Rail { selected: None });
        let autoplay = prefs.autoplay;
        let player = cx.new(|_| PlayerView::new(autoplay));
        let subs = vec![
            cx.subscribe_in(
                &top_bar,
                window,
                |this, _, ev: &TopBarEvent, window, cx| match ev {
                    TopBarEvent::Submit(q) => this.push_search(q.clone(), window, cx),
                    TopBarEvent::Back => this.back(window, cx),
                },
            ),
            cx.subscribe_in(&rail, window, |this, _, ev: &Navigate, window, cx| {
                this.replace_root(ev.0, window, cx)
            }),
            cx.subscribe_in(
                &player,
                window,
                |this, _, ev: &PlayerEvent, window, cx| match ev {
                    PlayerEvent::ToggleTheatre => this.toggle_theatre(cx),
                    PlayerEvent::ToggleFullscreen => window.toggle_fullscreen(),
                    PlayerEvent::PlayNext(v) => this.open_video(v.as_ref().clone(), window, cx),
                    PlayerEvent::AutoplayChanged(on) => {
                        this.prefs.autoplay = *on;
                        this.prefs.save();
                    }
                },
            ),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut app = NtApp {
            stack: Vec::new(),
            forward: Vec::new(),
            next_id: 1,
            images: ImageCache::default(),
            prefs,
            page_view,
            top_bar,
            rail,
            player,
            focus,
            geo: Geo {
                win_w: 1280.0,
                win_h: 800.0,
                scale: 1.0,
            },
            offset: 0.0,
            sb: ScrollbarUi::default(),
            theatre: false,
            fullscreen: false,
            frame_pending: false,
            last_precise: false,
            pending_marks: vec![marks::FIRST_FRAME],
            first_frame: true,
            menu: None,
            toast: None,
            chips_x: 0.0,
            chips_max: 0.0,
            chip_widths: Vec::new(),
            _subs: subs,
        };
        app.replace_root(Root::Home, window, cx);
        app.spawn_autopilot(window, cx);
        app
    }

    fn spawn_autopilot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !nt_core::autopilot::enabled() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            while let Some(cmd) = nt_core::autopilot::next_command().await {
                if this
                    .update_in(cx, |app, window, cx| app.run_command(cmd, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Something the page shows changed: redraw the page view (and the root, which positions the
    /// player over it).
    pub fn content_changed(&mut self, cx: &mut Context<Self>) {
        self.page_view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub fn on_image(&mut self, cx: &mut Context<Self>) {
        self.content_changed(cx);
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

    /// The watch page on top is about to be covered or dropped: stop playback, remember where.
    fn leave_watch(&mut self, cx: &mut Context<Self>) {
        if !self.top().is_some_and(|p| p.is_watch()) {
            return;
        }
        let stopped = self.player.update(cx, |p, cx| {
            p.set_next(None, cx);
            p.stop(cx)
        });
        if let Some((id, pos, dur)) = stopped {
            if let Some(w) = self.top_mut().and_then(|p| p.watch_mut())
                && w.video.id == id
            {
                w.resume_at = Some(pos).filter(|p| *p > 1.0);
            }
            self.prefs.set_position(&id, pos, dur);
            self.prefs.save();
        }
    }

    /// The page now on top is a watch page: play its video (again).
    fn enter_watch(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.top() else { return };
        let Some(w) = page.watch() else { return };
        let id = w.video.id.clone();
        let resume = w.resume_at.or_else(|| self.prefs.resume_at(&id));
        let next = page.feed.items.first().cloned();
        self.player.update(cx, |p, cx| {
            p.load(&id, resume, cx);
            p.set_next(next, cx);
        });
    }

    /// Top bar and rail follow the stack.
    fn sync_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let can_back = self.stack.len() > 1;
        let selected = if self.stack.len() == 1 {
            self.top().and_then(|p| p.root())
        } else {
            None
        };
        self.top_bar.update(cx, |t, cx| {
            if t.can_back != can_back {
                t.can_back = can_back;
                cx.notify();
            }
        });
        self.rail.update(cx, |r, cx| {
            if r.selected != selected {
                r.selected = selected;
                cx.notify();
            }
        });
        // The search box shows the query of the page on screen: a search page's own query, an
        // empty box on the root pages, and whatever was there on watch and channel pages.
        let query = match self.top().map(|p| &p.kind) {
            Some(Kind::Search { query }) => Some(query.clone()),
            Some(
                Kind::Home { .. } | Kind::Subscriptions { .. } | Kind::History | Kind::WatchLater,
            ) => Some(String::new()),
            _ => None,
        };
        if let Some(q) = query {
            self.top_bar.update(cx, |t, cx| t.set_query(&q, window, cx));
        }
        self.offset = self.top().map(|p| p.scroll.offset()).unwrap_or(0.0);
        self.menu = None;
        self.content_changed(cx);
    }

    pub fn push(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_watch(cx);
        self.forward.clear();
        let page = self.new_page(kind);
        let id = page.id;
        self.stack.push(page);
        // A long session must not grow memory without bound: past 30 pages, the oldest page
        // above the root is forgotten (Back then skips it).
        if self.stack.len() > MAX_STACK {
            self.stack.remove(1);
        }
        self.sb.last_activity = 0.0;
        self.start_load(id, cx);
        self.enter_watch(cx);
        self.sync_chrome(window, cx);
        window.focus(&self.focus, cx);
    }

    pub fn replace_root(&mut self, root: Root, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_watch(cx);
        self.stack.clear();
        self.forward.clear();
        if root != Root::Home {
            self.chips_x = 0.0;
        }
        let kind = match root {
            Root::Home => Kind::Home { topic: 0 },
            Root::Subscriptions => Kind::Subscriptions {
                channels: Vec::new(),
            },
            Root::History => Kind::History,
            Root::WatchLater => Kind::WatchLater,
        };
        self.push(kind, window, cx);
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stack.len() <= 1 {
            return;
        }
        self.leave_watch(cx);
        if let Some(p) = self.stack.pop() {
            self.forward.push(p);
            // Pages kept for Forward are bounded.
            if self.forward.len() > 20 {
                self.forward.remove(0);
            }
        }
        self.after_restore(window, cx);
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.forward.pop() else { return };
        self.leave_watch(cx);
        self.stack.push(p);
        self.after_restore(window, cx);
    }

    fn after_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Library pages may be stale after a visit elsewhere (a new Watch Later entry).
        if let Some(page) = self.top_mut()
            && matches!(page.kind, Kind::History | Kind::WatchLater)
        {
            let id = page.id;
            self.start_load(id, cx);
        }
        self.enter_watch(cx);
        self.sb.last_activity = 0.0;
        self.sync_chrome(window, cx);
        window.focus(&self.focus, cx);
    }

    pub fn select_topic(&mut self, topic: usize, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        if let Kind::Home { topic: t } = &mut page.kind {
            *t = topic.min(TOPICS.len().saturating_sub(1));
            page.feed = Feed::default();
            page.text.clear_titles();
            page.scroll.jump_to(0.0);
            let id = page.id;
            self.offset = 0.0;
            self.start_load(id, cx);
            self.content_changed(cx);
        }
    }

    pub fn open_video(&mut self, video: Video, window: &mut Window, cx: &mut Context<Self>) {
        library().record_watch(&video);
        let saved = library().is_saved(&video.id);
        let w = WatchState {
            video,
            info: None,
            comments: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_state: CommentsState::NotStarted,
            comments_req: 0,
            want_comments: false,
            desc_expanded: false,
            expanded_comments: Default::default(),
            subscribed: false,
            saved,
            resume_at: None,
        };
        self.push(Kind::Watch(Box::new(w)), window, cx);
    }

    pub fn open_channel(
        &mut self,
        channel_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if channel_id.is_empty() {
            return;
        }
        let subscribed = library().is_subscribed(&channel_id);
        self.push(
            Kind::Channel {
                id: channel_id,
                info: None,
                subscribed,
                desc_expanded: false,
            },
            window,
            cx,
        );
    }

    pub fn push_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.push(Kind::Search { query }, window, cx);
    }

    /// The same submit as pressing Enter in the field (the autopilot's Search lands here).
    pub fn submit_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.top_bar
            .update(cx, |t, cx| t.set_query(&query, window, cx));
        self.push_search(query, window, cx);
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
                // Drop answers for pages that are gone or were reloaded since.
                if app.stack.iter().any(|p| p.id == id && p.req == req) {
                    apply(app, id, result, cx);
                    if let Some(p) = app.page_mut(id) {
                        p.touch();
                    }
                    if app.is_top(id) {
                        app.content_changed(cx);
                    }
                }
            });
        })
        .detach();
    }

    fn bump_req(&mut self, id: PageId) -> Option<u64> {
        let p = self.page_mut(id)?;
        p.req = p.req.wrapping_add(1);
        p.touch();
        Some(p.req)
    }

    pub fn start_load(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(req) = self.bump_req(id) else { return };
        let Some(page) = self.page_mut(id) else {
            return;
        };
        page.status = Load::Loading;
        page.feed.loading_more = false;
        page.feed.more_error = None;
        match &mut page.kind {
            Kind::Home { topic } => {
                let t = TOPICS.get(*topic).map(|t| t.id).unwrap_or("all");
                let job = client().home(t);
                self.spawn_feed(id, req, job, marks::HOME_LOADED, "home feed", cx);
            }
            Kind::Search { query } => {
                let job = client().search(query);
                self.spawn_feed(id, req, job, marks::SEARCH_LOADED, "search", cx);
            }
            Kind::Channel { id: cid, .. } => {
                let job = client().channel(cid);
                self.spawn_job(id, req, job, cx, |app, id, r, _| {
                    let Some(page) = app.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            page.feed = Feed {
                                items: info.videos.items.clone(),
                                next: info.videos.next.clone(),
                                ..Default::default()
                            };
                            page.text.clear_titles();
                            if let Kind::Channel {
                                info: slot,
                                subscribed,
                                ..
                            } = &mut page.kind
                            {
                                *subscribed = library().is_subscribed(&info.id);
                                *slot = Some(Box::new(info));
                            }
                            page.status = Load::Ready;
                            app.mark_if_top(id, marks::CHANNEL_LOADED);
                        }
                        Err(e) => page.status = Load::Failed(Problem::from_error(&e, "channel")),
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
                                page.feed = Feed {
                                    items,
                                    ..Default::default()
                                };
                                page.text.clear_titles();
                                page.status = Load::Ready;
                            }
                            Err(e) => {
                                page.status = Load::Failed(Problem::from_error(&e, "subscriptions"))
                            }
                        }
                    });
                }
            }
            Kind::History => {
                page.feed = Feed {
                    items: library().history(),
                    ..Default::default()
                };
                page.status = Load::Ready;
            }
            Kind::WatchLater => {
                page.feed = Feed {
                    items: library().watch_later(),
                    ..Default::default()
                };
                page.status = Load::Ready;
            }
            Kind::Watch(w) => {
                let job = client().watch(&w.video.id);
                self.spawn_job(id, req, job, cx, |app, id, r, cx| {
                    let Some(page) = app.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            page.feed = Feed {
                                items: info.related.items.clone(),
                                next: info.related.next.clone(),
                                ..Default::default()
                            };
                            page.status = Load::Ready;
                            let next = page.feed.items.first().cloned();
                            let mut want = false;
                            if let Some(w) = page.watch_mut() {
                                w.subscribed = library().is_subscribed(&info.channel.id);
                                w.comments_next = info.comments.clone();
                                want = std::mem::take(&mut w.want_comments);
                                w.info = Some(info);
                            }
                            if app.is_top(id) {
                                app.player.update(cx, |p, cx| p.set_next(next, cx));
                            }
                            app.mark_if_top(id, marks::WATCH_LOADED);
                            if want {
                                app.load_comments(id, cx);
                            }
                        }
                        Err(e) => page.status = Load::Failed(Problem::from_error(&e, "watch page")),
                    }
                });
            }
        }
    }

    fn spawn_feed(
        &mut self,
        id: PageId,
        req: u64,
        job: Job<nt_core::Page<Video>>,
        mark: &'static str,
        what: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.spawn_job(id, req, job, cx, move |app, id, r, _| {
            let Some(page) = app.page_mut(id) else { return };
            match r {
                Ok(p) => {
                    page.feed = Feed {
                        items: p.items,
                        next: p.next,
                        ..Default::default()
                    };
                    page.text.clear_titles();
                    page.status = Load::Ready;
                    app.mark_if_top(id, mark);
                }
                Err(e) => page.status = Load::Failed(Problem::from_error(&e, what)),
            }
        });
    }

    pub fn load_more(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else {
            return;
        };
        let Some(next) = page.feed.next.clone() else {
            return;
        };
        if page.feed.loading_more {
            return;
        }
        page.feed.loading_more = true;
        page.feed.more_error = None;
        let req = page.req;
        page.touch();
        let job = client().more_videos(&next);
        self.spawn_job(id, req, job, cx, |app, id, r, _| {
            let Some(page) = app.page_mut(id) else { return };
            page.feed.loading_more = false;
            match r {
                Ok(p) => {
                    let known: std::collections::HashSet<String> =
                        page.feed.items.iter().map(|v| v.id.clone()).collect();
                    page.feed
                        .items
                        .extend(p.items.into_iter().filter(|v| !known.contains(&v.id)));
                    page.feed.next = p.next;
                    app.mark_if_top(id, marks::MORE_LOADED);
                }
                Err(e) => page.feed.more_error = Some(Problem::from_error(&e, "more videos")),
            }
        });
    }

    pub fn load_comments(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else {
            return;
        };
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
                self.content_changed(cx);
            }
            return;
        };
        w.comments_state = CommentsState::Loading;
        w.comments_req = w.comments_req.wrapping_add(1);
        let creq = w.comments_req;
        page.touch();
        let job = client().more_comments(&next);
        self.spawn_job(id, req, job, cx, move |app, id, r, _| {
            let Some(w) = app.page_mut(id).and_then(|p| p.watch_mut()) else {
                return;
            };
            if w.comments_req != creq {
                return;
            }
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
                Err(e) => {
                    w.comments_state = CommentsState::Failed(Problem::from_error(&e, "comments"))
                }
            }
        });
    }

    /// Retry only the page's own data (never restarts the video).
    pub fn retry_page(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.top().map(|p| p.id) {
            self.start_load(id, cx);
            self.content_changed(cx);
        }
    }

    pub fn retry_more(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.top_mut() {
            page.feed.more_error = None;
            let id = page.id;
            self.load_more(id, cx);
            self.content_changed(cx);
        }
    }

    pub fn retry_comments(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.top_mut() {
            let id = page.id;
            if let Some(w) = page.watch_mut() {
                w.comments_state = CommentsState::Idle;
            }
            self.load_comments(id, cx);
            self.content_changed(cx);
        }
    }

    pub fn toggle_subscribe(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        match &mut page.kind {
            Kind::Watch(w) => {
                if let Some(info) = &w.info {
                    w.subscribed = library().toggle_subscription(&info.channel);
                }
            }
            Kind::Channel {
                info: Some(info),
                subscribed,
                ..
            } => {
                *subscribed = library().toggle_subscription(&nt_core::ChannelInfo::as_ref(info));
            }
            _ => {}
        }
        page.touch();
        self.content_changed(cx);
    }

    pub fn toggle_watch_later(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.top_mut().and_then(|p| p.watch_mut()) {
            let mut v = w
                .info
                .as_ref()
                .map(|i| i.as_video())
                .unwrap_or_else(|| w.video.clone());
            if v.duration.is_none() {
                v.duration = w.video.duration.clone();
            }
            w.saved = library().toggle_watch_later(&v);
            self.content_changed(cx);
        }
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        library().clear_history();
        if let Some(id) = self.top().map(|p| p.id) {
            self.start_load(id, cx);
        }
        self.content_changed(cx);
    }

    fn toggle_theatre(&mut self, cx: &mut Context<Self>) {
        self.theatre = !self.theatre;
        let t = self.theatre;
        self.player.update(cx, |p, cx| {
            p.theatre = t;
            cx.notify();
        });
        self.content_changed(cx);
    }

    // ---- scrolling -------------------------------------------------------------------------

    /// Ask for a frame tick before the next frame. The tick advances the scroll animation and
    /// notifies the page, so the page and the player move in the same frame.
    pub fn kick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frame_pending {
            return;
        }
        self.frame_pending = true;
        let this = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = this.update(cx, |app, cx| app.frame_tick(window, cx));
        });
    }

    fn frame_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.frame_pending = false;
        let t = now();
        let scale = self.geo.scale.max(0.5) as f64;
        let Some(page) = self.stack.last_mut() else {
            return;
        };
        let off = page.scroll.tick(t);
        let animating = page.scroll.needs_frame();
        let snapped = (off * scale).round() / scale;
        if snapped != self.offset {
            self.offset = snapped;
            self.sb.last_activity = t;
            self.content_changed(cx);
        }
        if animating {
            self.kick(window, cx);
        } else {
            self.schedule_scrollbar_hide(cx);
        }
        self.check_triggers(cx);
    }

    fn activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sb.last_activity = now();
        // Scrolling closes the card menu, as on YouTube.
        self.close_menu(cx);
        self.kick(window, cx);
    }

    /// Mouse-wheel notches (the autopilot's `Scroll` lands here too).
    pub fn wheel_ticks(&mut self, ticks: f64, t: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.last_precise = false;
        if let Some(p) = self.top_mut() {
            p.scroll.wheel_ticks(ticks, t);
        }
        self.activity(window, cx);
    }

    /// Precise touchpad pixels (the autopilot's `ScrollPixels` lands here too).
    pub fn wheel_pixels(&mut self, px: f64, t: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.last_precise = true;
        if let Some(p) = self.top_mut() {
            p.scroll.wheel_pixels(px, t);
        }
        self.activity(window, cx);
    }

    pub fn on_scroll_event(
        &mut self,
        ev: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = now();
        match ev.delta {
            // GPUI's Wayland backend reports a notch as 3 lines, negated (value120/120 × 3 × −1).
            ScrollDelta::Lines(d) => {
                let ticks = -(d.y as f64) / 3.0;
                if ticks != 0.0 && ticks.is_finite() {
                    self.wheel_ticks(ticks, t, window, cx);
                }
            }
            ScrollDelta::Pixels(d) => {
                if ev.touch_phase == TouchPhase::Ended {
                    // Finger lift (axis_stop, via the patched backend). Chrome only sees
                    // axis_stop after finger frames, so never fling off wheel samples.
                    if !std::mem::take(&mut self.last_precise) {
                        return;
                    }
                    if let Some(p) = self.top_mut() {
                        p.scroll.fling_start(t);
                    }
                    self.activity(window, cx);
                } else {
                    // The backend scales wl_pointer.axis by −3; Chrome uses ×12.
                    let k = if ev.source == gpui::ScrollSource::Touch {
                        -1.0
                    } else {
                        -4.0
                    };
                    let px = f64::from(f32::from(d.y)) * k;
                    if px != 0.0 && px.is_finite() {
                        self.wheel_pixels(px, t, window, cx);
                    }
                }
            }
        }
    }

    /// Horizontal wheel over the chip bar.
    pub fn scroll_chips(&mut self, dx: f32, cx: &mut Context<Self>) {
        let x = (self.chips_x + dx).clamp(0.0, self.chips_max.max(0.0));
        if x != self.chips_x {
            self.chips_x = x;
            self.content_changed(cx);
        }
    }

    pub fn cancel_fling(&mut self) {
        let t = now();
        if let Some(p) = self.top_mut() {
            p.scroll.cancel_fling(t);
        }
    }

    pub fn scroll_drag_to(&mut self, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some((y0, off0)) = self.sb.drag else {
            return;
        };
        let home = self.top().is_some_and(|p| p.is_home());
        let vh = self.geo.viewport_h(home) as f64;
        if let Some(page) = self.top_mut() {
            let max = page.scroll.max_offset();
            let content = max + vh;
            let thumb = (vh * vh / content.max(1.0)).max(32.0);
            let track = (vh - thumb).max(1.0);
            page.scroll.jump_to(off0 + (y - y0) as f64 * max / track);
        }
        self.activity(window, cx);
    }

    /// Infinite scroll and the comments trigger, checked against where motion will end.
    pub fn check_triggers(&mut self, cx: &mut Context<Self>) {
        let home = self.top().is_some_and(|p| p.is_home());
        let vh = self.geo.viewport_h(home) as f64;
        let Some(page) = self.stack.last() else {
            return;
        };
        let Some((_, lay)) = &page.layout else { return };
        let id = page.id;
        let bottom = page.scroll.target() + vh;
        let more = page.status == Load::Ready
            && page.feed.next.is_some()
            && !page.feed.loading_more
            && page.feed.more_error.is_none()
            && (lay.main_end as f64) - bottom < 1.5 * vh;
        let comments = page.watch().and_then(|w| {
            let cy = lay.comments_y? as f64;
            let first = w.comments.is_empty()
                && w.comments_state == CommentsState::NotStarted
                && cy - bottom < vh;
            let later = w.comments_state == CommentsState::Idle
                && w.comments_next.is_some()
                && (lay.comments_end as f64) - bottom < 1.5 * vh;
            Some(first || later)
        });
        if more {
            self.load_more(id, cx);
            self.content_changed(cx);
        }
        if comments == Some(true) {
            self.load_comments(id, cx);
            self.content_changed(cx);
        }
    }

    fn schedule_scrollbar_hide(&mut self, cx: &mut Context<Self>) {
        if self.sb.hide_pending || now() - self.sb.last_activity > 1.2 {
            return;
        }
        self.sb.hide_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1300))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.sb.hide_pending = false;
                app.content_changed(cx);
            });
        })
        .detach();
    }

    pub fn scrollbar_visible(&self, t: f64) -> bool {
        self.sb.drag.is_some() || t - self.sb.last_activity < 1.0
    }

    // ---- keys -------------------------------------------------------------------------------

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if key == HIDE_CURSOR_KEY {
            cx.stop_propagation();
            return;
        }
        let m = ks.modifiers;
        if key == "escape" && self.menu.is_some() {
            self.close_menu(cx);
            cx.stop_propagation();
            return;
        }
        if self.top_bar.read(cx).is_focused(window, cx) {
            return;
        }
        let t = now();
        if m.alt && key == "left" {
            self.back(window, cx);
            return;
        }
        if m.alt && key == "right" {
            self.go_forward(window, cx);
            return;
        }
        if key == "/" && !m.control && !m.alt && !m.platform {
            self.top_bar.update(cx, |b, cx| b.focus_search(window, cx));
            cx.stop_propagation();
            return;
        }
        if m.control || m.alt || m.platform {
            return;
        }
        if key == "escape" {
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            }
            return;
        }
        let watch = self.top().is_some_and(|p| p.is_watch());
        if watch && self.watch_key(key, m.shift, window, cx) {
            self.cancel_fling();
            cx.stop_propagation();
            return;
        }
        let home = self.top().is_some_and(|p| p.is_home());
        let vh = self.geo.viewport_h(home) as f64;
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
                self.activity(window, cx);
                cx.stop_propagation();
            }
            // Any other key stops a fling, as in Chrome.
            None => s.cancel_fling(t),
        }
    }

    /// YouTube's player keys. Returns whether the key was used.
    fn watch_key(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let p = self.player.clone();
        match key {
            "space" | "k" => p.update(cx, |p, cx| p.toggle_pause(cx)),
            "j" => p.update(cx, |p, cx| p.seek_by(-10.0, cx)),
            "l" => p.update(cx, |p, cx| p.seek_by(10.0, cx)),
            "left" => p.update(cx, |p, cx| p.seek_by(-5.0, cx)),
            "right" => p.update(cx, |p, cx| p.seek_by(5.0, cx)),
            "up" => p.update(cx, |p, cx| p.nudge_volume(5.0, cx)),
            "down" => p.update(cx, |p, cx| p.nudge_volume(-5.0, cx)),
            "m" => p.update(cx, |p, cx| p.toggle_mute(cx)),
            "f" => window.toggle_fullscreen(),
            "t" => self.toggle_theatre(cx),
            "home" => p.update(cx, |p, cx| p.seek_fraction(0.0, cx)),
            "end" => p.update(cx, |p, cx| p.seek_fraction(0.999, cx)),
            "," if !shift => p.update(cx, |p, cx| p.frame_step(false, cx)),
            "." if !shift => p.update(cx, |p, cx| p.frame_step(true, cx)),
            "<" | "," => p.update(cx, |p, cx| p.step_speed(false, cx)),
            ">" | "." => p.update(cx, |p, cx| p.step_speed(true, cx)),
            "n" if shift => p.update(cx, |p, cx| p.play_next(cx)),
            k if k.len() == 1 && k.as_bytes().first().is_some_and(|b| b.is_ascii_digit()) => {
                let d = k
                    .as_bytes()
                    .first()
                    .map(|b| (b - b'0') as f64)
                    .unwrap_or(0.0);
                p.update(cx, |p, cx| p.seek_fraction(d / 10.0, cx));
            }
            _ => return false,
        }
        true
    }

    // ---- autopilot -------------------------------------------------------------------------

    fn run_command(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        let t = now();
        match cmd {
            Command::Home { topic } => {
                self.replace_root(Root::Home, window, cx);
                if let Some(topic) = topic.and_then(|id| TOPICS.iter().position(|x| x.id == id))
                    && topic != 0
                {
                    self.select_topic(topic, cx);
                }
            }
            Command::Search { query } => self.submit_search(query, window, cx),
            Command::OpenVideo { index } => {
                if let Some(v) = self.top().and_then(|p| p.feed.items.get(index)).cloned() {
                    self.open_video(v, window, cx);
                }
            }
            Command::OpenChannel => {
                let id = self.top().and_then(|p| p.watch()).and_then(|w| {
                    w.info
                        .as_ref()
                        .map(|i| i.channel.id.clone())
                        .filter(|s| !s.is_empty())
                        .or_else(|| w.video.channel_id.clone())
                });
                if let Some(id) = id {
                    self.open_channel(id, window, cx);
                }
            }
            Command::Back => self.back(window, cx),
            Command::Scroll { ticks } => self.wheel_ticks(ticks, t, window, cx),
            Command::ScrollPixels { px } => self.wheel_pixels(px, t, window, cx),
            Command::Fling { velocity } => {
                if let Some(p) = self.top_mut() {
                    p.scroll.fling(velocity, t);
                }
                self.activity(window, cx);
            }
            Command::ScrollToTop => {
                if let Some(p) = self.top_mut() {
                    p.scroll.jump_to(0.0);
                }
                self.activity(window, cx);
            }
            Command::TogglePause => self.player.update(cx, |p, cx| p.toggle_pause(cx)),
            Command::LoadMoreComments => {
                if let Some(id) = self.top().map(|p| p.id) {
                    self.load_comments(id, cx);
                    self.content_changed(cx);
                }
            }
            Command::Quit => self.quit(cx),
        }
    }

    pub fn quit(&mut self, cx: &mut Context<Self>) {
        self.leave_watch(cx);
        cx.quit();
    }

    // ---- frame ----------------------------------------------------------------------------

    /// Lay out the top page if its geometry or data changed. Returns whether it did.
    fn update_layout(&mut self, window: &mut Window) -> bool {
        let geo = self.geo;
        let theatre = self.theatre;
        let Some(page) = self.stack.last_mut() else {
            return false;
        };
        let key = LayoutKey {
            geo,
            version: page.version,
            theatre,
        };
        if page.layout.as_ref().is_some_and(|(k, _)| *k == key) {
            return false;
        }
        let mut m = WindowMeasure { window };
        let lay = build_layout(page, &geo, theatre, &mut m);
        let home = page.is_home();
        let vh = geo.viewport_h(home) as f64;
        page.scroll.set_viewport(vh);
        page.scroll.set_max_offset(lay.content_h as f64 - vh);
        page.layout = Some((key, lay));
        // A shrinking page may have clamped the offset.
        self.offset = page.scroll.offset();
        true
    }
}

struct WindowMeasure<'a> {
    window: &'a mut Window,
}

impl Measure for WindowMeasure<'_> {
    fn lines(&mut self, text: &str, size: f32, weight: u16, width: f32) -> usize {
        if text.is_empty() {
            return 0;
        }
        let mut font = self.window.text_style().font();
        font.weight = gpui::FontWeight(weight as f32);
        let run = gpui::TextRun {
            len: text.len(),
            font,
            color: crate::theme::text(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        match self.window.text_system().shape_text(
            text.to_string().into(),
            px(size),
            &[run],
            Some(px(width)),
            None,
        ) {
            Ok(lines) => lines.iter().map(|l| 1 + l.wrap_boundaries().len()).sum(),
            Err(_) => text.lines().count().max(1),
        }
    }
}

impl Render for NtApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.images.begin_frame();
        let vp = window.viewport_size();
        let geo = Geo {
            win_w: f32::from(vp.width).max(1.0),
            win_h: f32::from(vp.height).max(1.0),
            scale: window.scale_factor(),
        };
        if geo != self.geo {
            self.geo = geo;
            let compact = geo.win_w < SEARCH_COLLAPSE;
            // The top bar's bounds change with the window, so its cached view re-renders anyway.
            self.top_bar.update(cx, |t, _| t.compact = compact);
        }
        let fullscreen = window.is_fullscreen();
        if fullscreen != self.fullscreen {
            self.fullscreen = fullscreen;
            self.player.update(cx, |p, _| p.fullscreen = fullscreen);
        }
        if self.update_layout(window) {
            cx.defer_in(window, |this, _, cx| this.check_triggers(cx));
        }

        let watch = self.top().is_some_and(|p| p.is_watch());
        let rail = geo.rail(watch) && !fullscreen;
        let cx0 = geo.content_x(watch);
        let cw = geo.content_w(watch);
        let ch = (geo.win_h - TOP_BAR_H).max(1.0);

        let mut root = div()
            .id("nt-root")
            .size_full()
            .relative()
            .bg(bg())
            .font_family("Roboto")
            .text_color(text())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit(cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, _, cx| this.quit(cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.back(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.go_forward(window, cx)),
            )
            // Wheel over the top bar, rail or chips scrolls the page, as in a browser.
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                this.on_scroll_event(ev, window, cx)
            }));

        if !fullscreen {
            root = root.child(
                self.page_view.clone().cached(
                    StyleRefinement::default()
                        .absolute()
                        .left(px(cx0))
                        .top(px(TOP_BAR_H))
                        .w(px(cw))
                        .h(px(ch)),
                ),
            );
        }

        // The player sits over the page's player block, clipped to the page area.
        if watch {
            let rect = self
                .top()
                .and_then(|p| p.layout.as_ref())
                .and_then(|(_, l)| l.player);
            if fullscreen {
                root = root.child(
                    div().absolute().inset_0().bg(black()).child(
                        self.player
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                );
            } else if let Some((x, y, w, h)) = rect {
                let top = y - self.offset as f32;
                // Not drawn while scrolled out of view: its frames then cost nothing.
                if top + h > 0.0 && top < ch {
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(cx0))
                            .top(px(TOP_BAR_H))
                            .w(px(cw))
                            .h(px(ch))
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(x))
                                    .top(px(top))
                                    .w(px(w))
                                    .h(px(h))
                                    .child(
                                        self.player
                                            .clone()
                                            .cached(StyleRefinement::default().size_full()),
                                    ),
                            ),
                    );
                }
            }
        }

        if rail {
            root = root.child(
                self.rail.clone().cached(
                    StyleRefinement::default()
                        .absolute()
                        .left_0()
                        .top(px(TOP_BAR_H))
                        .w(px(RAIL_W))
                        .h(px(ch)),
                ),
            );
        }
        if !fullscreen {
            root = root.child(
                self.top_bar.clone().cached(
                    StyleRefinement::default()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(geo.win_w))
                        .h(px(TOP_BAR_H)),
                ),
            );
        }
        root = root.children(self.card_menu(cx));
        if let Some((msg, _)) = self.toast {
            root = root.child(
                div()
                    .absolute()
                    .left(px(24.0))
                    .bottom(px(24.0))
                    .px(px(16.0))
                    .py(px(14.0))
                    .rounded(px(8.0))
                    .bg(surface())
                    .shadow_lg()
                    .child(txt(msg, 14.0, 20.0, W400, text())),
            );
        }

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
        root.child(hook)
    }
}

impl NtApp {
    /// Open the card menu (right-click or "⋮") for an item of a page's list, at the pointer.
    pub fn open_card_menu(
        &mut self,
        page: PageId,
        index: usize,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) {
        let v = self
            .stack
            .iter()
            .find(|p| p.id == page)
            .and_then(|p| p.feed.items.get(index))
            .cloned();
        if let Some(video) = v {
            self.menu = Some(CardMenu { video, x, y });
            cx.notify();
        }
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn copy_link(&mut self, id: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!(
            "https://www.youtube.com/watch?v={id}"
        )));
        self.close_menu(cx);
        self.show_toast("Link copied", cx);
    }

    fn show_toast(&mut self, text: &'static str, cx: &mut Context<Self>) {
        let at = now();
        self.toast = Some((text, at));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |app, cx| {
                if app.toast.is_some_and(|(_, t)| t == at) {
                    app.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn card_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let v = menu.video.clone();
        let saved = library().is_saved(&v.id);
        let channel = v.channel_id.clone().filter(|c| !c.is_empty());
        let rows = 3 + usize::from(channel.is_some());
        const W: f32 = 264.0;
        let h = 16.0 + rows as f32 * 40.0;
        // Keep the menu on screen: flip left/up when it would run past the window edge.
        let (gw, gh) = (self.geo.win_w, self.geo.win_h);
        let x = if menu.x + W > gw - 8.0 {
            menu.x - W
        } else {
            menu.x
        };
        let y = if menu.y + h > gh - 8.0 {
            menu.y - h
        } else {
            menu.y
        };
        let (x, y) = (
            x.clamp(8.0, (gw - W - 8.0).max(8.0)),
            y.clamp(8.0, (gh - h - 8.0).max(8.0)),
        );
        let item = |id: &'static str, name: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(40.0))
                .px(px(16.0))
                .flex()
                .items_center()
                .gap(px(16.0))
                .cursor_pointer()
                .hover(|s| s.bg(fill()))
                .child(crate::icons::icon(name, 24.0, text()))
                .child(txt(label, 14.0, 20.0, W400, text()))
        };
        let (v_play, v_later, id_link) = (v.clone(), v.clone(), v.id.clone());
        let mut panel = div()
            .id("card-menu")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(W))
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(menu_bg())
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                item("menu-play", "play_arrow", "Play").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.menu = None;
                        this.open_video(v_play.clone(), window, cx);
                    },
                )),
            )
            .child(
                item(
                    "menu-later",
                    if saved { "delete" } else { "schedule" },
                    if saved {
                        "Remove from Watch later"
                    } else {
                        "Save to Watch later"
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let now_saved = library().toggle_watch_later(&v_later);
                    this.menu = None;
                    this.show_toast(
                        if now_saved {
                            "Saved to Watch later"
                        } else {
                            "Removed from Watch later"
                        },
                        cx,
                    );
                    // A Watch later page on screen shows the change at once.
                    if let Some(page) = this.top_mut()
                        && matches!(page.kind, Kind::WatchLater)
                    {
                        let id = page.id;
                        this.start_load(id, cx);
                    }
                    this.content_changed(cx);
                })),
            )
            .child(
                item("menu-link", "share", "Copy link")
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_link(&id_link, cx))),
            );
        if let Some(cid) = channel {
            panel = panel.child(
                item("menu-channel", "subscriptions", "Go to channel").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.menu = None;
                        this.open_channel(cid.clone(), window, cx);
                    },
                )),
            );
        }
        Some(
            gpui::deferred(
                // A full-window catcher: a click outside (either button) closes the menu and does
                // nothing else; a wheel closes it and scrolls the page (it bubbles to the root).
                div()
                    .id("menu-catcher")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.close_menu(cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.close_menu(cx);
                        }),
                    )
                    // The catcher hides the page from the pointer, so it forwards the wheel:
                    // scrolling closes the menu and scrolls the page.
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                        cx.stop_propagation();
                        this.on_scroll_event(ev, window, cx);
                    }))
                    .child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
