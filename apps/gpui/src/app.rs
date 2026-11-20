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
use crate::probe::{self, Layer, target};
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
use nt_core::{ChannelTab, Job, SearchFilter, TOPICS, Video, client, library};
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

/// A window-wide layer over the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    None,
    /// The `?` sheet.
    Shortcuts,
    Filters,
    Playlist,
    Queue,
    About,
    /// The guide as a drawer over the page (Watch, narrow windows).
    Guide,
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
    pub preview: Entity<crate::preview::HoverPreview>,
    pub hover_video: Option<(PageId, usize)>,
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
    pub playlist: crate::playlist::PlaylistPopup,
    pub about: Option<(Load, nt_core::ChannelAbout)>,
    pub overlay: Overlay,
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
        if std::env::var("NT_PROBE").as_deref() == Ok("1") {
            cx.set_global(probe::Probe::default());
        }
        let prefs = Prefs::load();
        let me = cx.entity().downgrade();
        let page_view = cx.new(|_| PageView { app: me });
        let top_bar = cx.new(|cx| TopBar::new(window, cx));
        let rail = cx.new(|_| Rail { selected: None });
        let autoplay = prefs.autoplay;
        let preview = cx.new(|_| crate::preview::HoverPreview::default());
        let player = cx.new(|_| PlayerView::new(autoplay));
        let subs = vec![
            cx.on_app_quit(|a, cx| {
                a.close_playback(cx);
                std::future::ready(())
            }),
            cx.subscribe_in(
                &top_bar,
                window,
                |this, _, ev: &TopBarEvent, window, cx| match ev {
                    TopBarEvent::Submit(q) => this.push_search(q.clone(), window, cx),
                    TopBarEvent::Back => this.back(window, cx),
                    TopBarEvent::Guide => this.toggle_guide(cx),
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
                    PlayerEvent::ToggleFullscreen => this.toggle_fullscreen(window, cx),
                    PlayerEvent::PlayNext(v) => this.advance(v.as_ref().clone(), window, cx),
                    PlayerEvent::PlayPrevious(v) => {
                        this.play_playlist_item(v.as_ref().clone(), window, cx)
                    }
                    PlayerEvent::AutoplayChanged(on) => {
                        this.prefs.autoplay = *on;
                        this.prefs.save();
                    }
                    PlayerEvent::Toast(msg) => this.show_toast(msg, cx),
                    PlayerEvent::ShowChapters => this.show_chapters(window, cx),
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
            preview,
            hover_video: None,
            focus,
            geo: Geo {
                win_w: 1280.0,
                win_h: 800.0,
                scale: 1.0,
                guide: false,
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
            playlist: Default::default(),
            about: None,
            overlay: Overlay::None,
            toast: None,
            chips_x: 0.0,
            chips_max: 0.0,
            chip_widths: Vec::new(),
            _subs: subs,
        };
        app.replace_root(Root::Home, window, cx);
        if let Some(resume) = library()
            .last_played()
            .filter(|v| v.position.is_finite() && v.position > 0.0)
            && let Some(page) = app.top_mut()
        {
            let video = library()
                .history()
                .into_iter()
                .find(|v| v.id == resume.id)
                .unwrap_or_else(|| Video {
                    thumbnail: Video::thumbnail_url(&resume.id),
                    id: resume.id,
                    title: resume.title,
                    ..Video::default()
                });
            page.resume = Some((video, resume.position));
        }
        app.spawn_autopilot(window, cx);
        probe::spawn(window, cx);
        // Test-only (`NT_TEST_THEATRE=1`): start in theatre mode, for input-free layout checks.
        if std::env::var("NT_TEST_THEATRE").as_deref() == Ok("1") {
            app.theatre = true;
            app.player.update(cx, |p, _| p.theatre = true);
        }
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

    pub fn hover_card(
        &mut self,
        row: (PageId, usize),
        id: &str,
        width: f32,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.is_top(row.0) || self.overlay != Overlay::None || self.menu.is_some() {
            return;
        }
        let already = self.hover_video == Some(row);
        if active && !already {
            self.hover_video = Some(row);
            let width = (width * self.geo.scale).round().clamp(64.0, 640.0) as u32;
            self.preview
                .update(cx, |p, cx| p.request(id.to_string(), width, cx));
            self.content_changed(cx);
        } else if !active && already {
            self.clear_hover(cx);
        }
    }

    pub fn clear_hover(&mut self, cx: &mut Context<Self>) {
        if self.hover_video.take().is_some() {
            self.preview.update(cx, |p, cx| p.stop(cx));
            self.content_changed(cx);
        }
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
        self.stack
            .iter_mut()
            .chain(&mut self.forward)
            .find(|p| p.id == id)
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
    fn leave_watch(&mut self, to_watch: bool, cx: &mut Context<Self>) {
        if !self.top().is_some_and(|p| p.is_watch()) {
            return;
        }
        let stopped = self.player.update(cx, |p, cx| {
            if !to_watch {
                p.set_next(None, cx);
            }
            p.stop(!to_watch, cx)
        });
        if let Some((id, pos, dur)) = stopped {
            if let Some(w) = self.top_mut().and_then(|p| p.watch_mut())
                && w.video.id == id
                && dur > 0.0
            {
                w.resume_at = Some(pos.max(0.0));
            }
            self.prefs.set_position(&id, pos, dur);
            self.prefs.save();
        }
    }

    /// The page now on top is a watch page: play its video (again).
    fn enter_watch(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(page) = self.top() else { return };
        let Some(watch) = page.watch() else { return };
        let id = watch.video.id.clone();
        let thumb = watch.video.thumbnail.clone();
        let resume = watch.resume_at.or_else(|| self.prefs.resume_at(&id));
        let (next, previous) = successors(page);
        let (_, _, w, h) = watch_geo(&self.geo, self.theatre).player;
        let (w, h) = if self.fullscreen {
            (self.geo.win_w, self.geo.win_h)
        } else {
            (w, h)
        };
        let (w, h) = (w * self.geo.scale, h * self.geo.scale);
        let fallback = self.images.ready(&thumb);
        let extras = watch
            .info
            .as_ref()
            .map(|i| (i.chapters.clone(), i.heatmap.clone()));
        self.player.update(cx, |p, cx| {
            p.set_target((w.round() as u32, h.round() as u32));
            p.attach_hardware(window, cx);
            p.set_poster_fallback(fallback);
            p.load(&id, resume, Some((&thumb, w)), cx);
            p.set_next(next, cx);
            p.set_previous(previous, cx);
            if let Some((chapters, heat)) = extras {
                p.set_marks(chapters, heat, cx);
            }
        });
    }

    /// Top bar and rail follow the stack.
    fn sync_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_hover(cx);
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
            Some(Kind::Search { query, .. }) => Some(query.clone()),
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
        self.overlay = Overlay::None;
        self.content_changed(cx);
    }

    pub fn push(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_watch(matches!(kind, Kind::Watch(_)), cx);
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
        // Playback and its resolver get the first opportunity to use core's request gate.
        self.enter_watch(window, cx);
        self.start_load(id, cx);
        self.sync_chrome(window, cx);
        window.focus(&self.focus, cx);
    }

    pub fn replace_root(&mut self, root: Root, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_watch(false, cx);
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
        let to_watch = self
            .stack
            .get(self.stack.len() - 2)
            .is_some_and(Page::is_watch);
        self.leave_watch(to_watch, cx);
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
        self.leave_watch(p.is_watch(), cx);
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
        self.enter_watch(window, cx);
        self.sb.last_activity = 0.0;
        self.sync_chrome(window, cx);
        window.focus(&self.focus, cx);
    }

    pub fn select_topic(&mut self, topic: usize, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        if let Kind::Home { topic: t } = &mut page.kind {
            let count = if page.home_chips.is_empty() {
                TOPICS.len()
            } else {
                page.home_chips.len()
            };
            *t = topic.min(count.saturating_sub(1));
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
        self.open_video_at(video, None, window, cx);
    }

    /// Open a video, optionally at `start` seconds (a transcript match, a timestamp link).
    pub fn open_video_at(
        &mut self,
        video: Video,
        start: Option<f64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_from_playlist(video, start, None, window, cx);
    }

    pub fn open_from_playlist(
        &mut self,
        mut video: Video,
        start: Option<f64>,
        playlist: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::player::trace("navigate", &video.id);
        // The moment belongs to the search row, not to the video.
        video.moment = None;
        library().record_watch(&video);
        let saved = library().is_saved(&video.id);
        let w = WatchState {
            video,
            playlist,
            playlist_loading: false,
            playlist_error: false,
            info: None,
            comments: Vec::new(),
            comments_next: None,
            comments_total: None,
            comments_sort: Vec::new(),
            comments_replace: None,
            sort_open: false,
            replies: Default::default(),
            related_selected: None,
            related_pending: None,
            related_failed: None,
            comments_state: CommentsState::NotStarted,
            comments_req: 0,
            want_comments: false,
            desc_expanded: false,
            expanded_comments: Default::default(),
            subscribed: false,
            saved,
            resume_at: start,
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
                tab: ChannelTab::Videos,
                sort: None,
            },
            window,
            cx,
        );
    }

    pub fn channel_tab(&mut self, tab: ChannelTab, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        let Kind::Channel {
            tab: current, sort, ..
        } = &mut page.kind
        else {
            return;
        };
        if *current == tab {
            return;
        }
        *current = tab;
        *sort = None;
        page.scroll.jump_to(0.0);
        let id = page.id;
        self.start_load(id, cx);
        self.content_changed(cx);
    }

    pub fn channel_sort(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(page) = self.top_mut() else { return };
        let Kind::Channel { sort, .. } = &mut page.kind else {
            return;
        };
        if *sort == Some(index) {
            return;
        }
        *sort = Some(index);
        let id = page.id;
        self.start_load(id, cx);
        self.content_changed(cx);
    }

    pub fn open_channel_about(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.top() else { return };
        let Kind::Channel {
            info: Some(info), ..
        } = &p.kind
        else {
            return;
        };
        let id = p.id;
        let known = nt_core::ChannelAbout {
            description: info.description.clone(),
            links: info.links.clone(),
        };
        let cursor = info.about.clone();
        let load = if cursor.is_some() {
            Load::Loading
        } else {
            Load::Ready
        };
        self.about = Some((load, known));
        self.set_overlay(Overlay::About, cx);
        let Some(cursor) = &cursor else { return };
        let job = client().channel_about(cursor);
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |a, cx| {
                if a.overlay != Overlay::About || !a.is_top(id) {
                    return;
                }
                let Some((status, info)) = &mut a.about else {
                    return;
                };
                match result {
                    Ok(data) => {
                        *status = Load::Ready;
                        *info = data;
                    }
                    Err(e) => *status = Load::Failed(Problem::from_error(&e, "channel about")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn push_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.push(
            Kind::Search {
                query,
                view: Box::default(),
            },
            window,
            cx,
        );
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
        job: impl Future<Output = nt_core::Result<T>> + 'static,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut NtApp, PageId, nt_core::Result<T>, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                // Drop answers for pages that are gone or were reloaded since.
                if app.page_mut(id).is_some_and(|p| p.req == req) {
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
                let chip = page.home_chips.get(*topic);
                if let Some(cursor) = chip.and_then(|chip| chip.next.as_ref()) {
                    let job = client().more_videos(cursor);
                    self.spawn_feed(id, req, job, marks::HOME_LOADED, "home feed", cx);
                } else if *topic == 0 || chip.is_some() {
                    let job = client().home_feed();
                    self.spawn_job(id, req, job, cx, |a, id, result, _| {
                        let Some(page) = a.page_mut(id) else { return };
                        match result {
                            Ok(f) => {
                                if let Kind::Home { topic } = &mut page.kind {
                                    *topic = f.chips.iter().position(|c| c.selected).unwrap_or(0);
                                }
                                page.home_chips = f.chips;
                                page.feed = Feed {
                                    items: f.videos.items,
                                    next: f.videos.next,
                                    ..Default::default()
                                };
                                page.status = Load::Ready;
                                page.text.clear_titles();
                                if a.is_top(id) {
                                    a.chip_widths.clear();
                                    a.chips_x = 0.0;
                                }
                                a.mark_if_top(id, marks::HOME_LOADED);
                            }
                            Err(e) => {
                                page.status = Load::Failed(Problem::from_error(&e, "home feed"))
                            }
                        }
                    });
                } else {
                    let t = TOPICS.get(*topic).map_or("all", |t| t.id);
                    self.spawn_feed(
                        id,
                        req,
                        client().home(t),
                        marks::HOME_LOADED,
                        "home feed",
                        cx,
                    );
                }
            }
            Kind::Search { query, view } => {
                let query = query.clone();
                let filter = view.filter.clone();
                let job = client().search_filtered(&query, &filter);
                let job = async move {
                    match job.await {
                        // Early replay fixtures recorded only the video-filtered search.
                        Err(nt_core::Error::Fixture(_)) if filter == SearchFilter::All => {
                            client().search_page(&query).await
                        }
                        result => result,
                    }
                };
                self.spawn_job(id, req, job, cx, |app, id, r, _| {
                    let Some(page) = app.page_mut(id) else { return };
                    let Kind::Search { view, .. } = &mut page.kind else {
                        return;
                    };
                    match r {
                        Ok(sp) => {
                            view.update(&mut page.feed, sp);
                            page.text.clear_titles();
                            page.status = Load::Ready;
                            app.mark_if_top(id, marks::SEARCH_LOADED);
                        }
                        Err(e) => page.status = Load::Failed(Problem::from_error(&e, "search")),
                    }
                });
            }
            Kind::Channel {
                id: cid,
                tab,
                info,
                sort,
                ..
            } => {
                let tab = *tab;
                let sort_cursor = sort.and_then(|i| info.as_ref()?.video_sort.get(i)?.next.clone());
                if tab == ChannelTab::Videos
                    && let Some(cursor) = sort_cursor
                {
                    self.spawn_feed(
                        id,
                        req,
                        client().more_videos(&cursor),
                        marks::CHANNEL_LOADED,
                        "sorted channel videos",
                        cx,
                    );
                    return;
                }
                let job = client().channel_tab(cid, tab);
                self.spawn_job(id, req, job, cx, move |app, id, r, _| {
                    let Some(page) = app.page_mut(id) else { return };
                    match r {
                        Ok(info) => {
                            let mut videos = info.videos.items.clone();
                            if tab == ChannelTab::Home
                                && let Some(featured) = &info.featured
                                && videos.iter().all(|v| v.id != featured.id)
                            {
                                videos.insert(0, featured.clone());
                            }
                            page.feed = Feed {
                                items: videos,
                                next: if tab == ChannelTab::Playlists {
                                    info.playlists.next.clone()
                                } else {
                                    info.videos.next.clone()
                                },
                                ..Default::default()
                            };
                            page.text.clear_titles();
                            if let Kind::Channel {
                                info: slot,
                                subscribed,
                                sort,
                                ..
                            } = &mut page.kind
                            {
                                *subscribed = library().is_subscribed(&info.id);
                                *sort = info.video_sort.iter().position(|c| c.selected);
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
                let job = match &w.playlist {
                    Some(playlist) => client().watch_in_playlist(&w.video.id, playlist),
                    None => client().watch(&w.video.id),
                };
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
                            let mut want = false;
                            if let Some(w) = page.watch_mut() {
                                w.subscribed = library().is_subscribed(&info.channel.id);
                                w.comments_next = info.comments.clone();
                                want = std::mem::take(&mut w.want_comments);
                                w.info = Some(info);
                            }
                            if app.is_top(id) {
                                let extras = page_extras(app.page_mut(id));
                                app.sync_next(id, cx);
                                app.player.update(cx, |p, cx| {
                                    if let Some((chapters, heat)) = extras {
                                        p.set_marks(chapters, heat, cx);
                                    }
                                });
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

    pub fn filter_search(
        &mut self,
        filter: SearchFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_overlay(Overlay::None, cx);
        let Some(p) = self.top_mut() else { return };
        let Kind::Search { view, .. } = &mut p.kind else {
            return;
        };
        if view.filter == filter && p.status == Load::Ready {
            return;
        }
        view.filter = filter;
        p.scroll.jump_to(0.0);
        let id = p.id;
        self.offset = 0.0;
        self.start_load(id, cx);
        window.focus(&self.focus, cx);
        self.content_changed(cx);
    }

    fn more_search(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(p) = self.page_mut(id) else { return };
        let Kind::Search { view, .. } = &p.kind else {
            return;
        };
        let Some(next) = p.feed.next.clone().or_else(|| view.transcript_next.clone()) else {
            return;
        };
        if p.feed.loading_more {
            return;
        }
        let transcript = p.feed.next.is_none();
        p.feed.loading_more = true;
        p.feed.more_error = None;
        let req = p.req;
        p.touch();
        if transcript {
            let job = client().more_videos(&next);
            self.spawn_job(id, req, job, cx, move |app, id, result, _| {
                let Some(p) = app.page_mut(id) else { return };
                let Kind::Search { view, .. } = &mut p.kind else {
                    return;
                };
                p.feed.loading_more = false;
                match result {
                    Ok(page) => {
                        for video in page.items {
                            view.transcripts.push(p.feed.items.len());
                            p.feed.items.push(video);
                        }
                        view.transcript_next = page.next.filter(|c| c != &next);
                        app.mark_if_top(id, marks::MORE_LOADED);
                    }
                    Err(e) => {
                        p.feed.more_error = Some(Problem::from_error(&e, "more transcript matches"))
                    }
                }
            });
        } else {
            let job = client().more_search(&next);
            self.spawn_job(id, req, job, cx, move |app, id, result, _| {
                let Some(p) = app.page_mut(id) else { return };
                let Kind::Search { view, .. } = &mut p.kind else {
                    return;
                };
                p.feed.loading_more = false;
                match result {
                    Ok(page) => {
                        view.extend(&mut p.feed, page.entries, page.results.items);
                        p.feed.next = page.results.next.filter(|c| c != &next);
                        app.mark_if_top(id, marks::MORE_LOADED);
                    }
                    Err(e) => {
                        p.feed.more_error = Some(Problem::from_error(&e, "more search results"))
                    }
                }
            });
        }
    }

    pub fn load_more(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else {
            return;
        };
        if matches!(page.kind, Kind::Search { .. }) {
            self.more_search(id, cx);
            return;
        }
        let Some(next) = page.feed.next.clone() else {
            return;
        };
        if page.feed.loading_more || page.watch().is_some_and(|w| w.related_pending.is_some()) {
            return;
        }
        page.feed.loading_more = true;
        page.feed.more_error = None;
        let req = page.req;
        page.touch();
        if matches!(
            page.kind,
            Kind::Channel {
                tab: ChannelTab::Playlists,
                ..
            }
        ) {
            let job = client().more_playlists(&next);
            self.spawn_job(id, req, job, cx, move |a, id, result, _| {
                let Some(p) = a.page_mut(id) else { return };
                p.feed.loading_more = false;
                match result {
                    Ok(page) => {
                        if let Kind::Channel {
                            info: Some(info), ..
                        } = &mut p.kind
                        {
                            info.playlists.items.extend(page.items);
                        }
                        p.feed.next = page.next.filter(|c| c != &next);
                        a.mark_if_top(id, marks::MORE_LOADED);
                    }
                    Err(e) => p.feed.more_error = Some(Problem::from_error(&e, "more playlists")),
                }
            });
            return;
        }
        let job = client().more_videos(&next);
        self.spawn_job(id, req, job, cx, move |app, id, r, _| {
            let Some(page) = app.page_mut(id) else { return };
            if page.feed.next.as_ref() != Some(&next)
                || !page.feed.loading_more
                || page
                    .watch()
                    .is_some_and(|w| w.related_pending.is_some() || w.related_failed.is_some())
            {
                return;
            }
            page.feed.loading_more = false;
            match r {
                Ok(p) => {
                    // Transcript matches can hold several moments of one video.
                    let key = |v: &Video| {
                        let at = v.moment.as_ref().map(|m| (m.start_secs * 1000.0) as i64);
                        (v.id.clone(), at)
                    };
                    let known: std::collections::HashSet<(String, Option<i64>)> =
                        page.feed.items.iter().map(key).collect();
                    page.feed
                        .items
                        .extend(p.items.into_iter().filter(|v| !known.contains(&key(v))));
                    page.feed.next = p.next.filter(|c| c != &next);
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
        let replacement = w.comments_replace;
        let cursor = match replacement {
            Some(index) => w.comments_sort.get(index).map(|s| s.next.clone()),
            None => w.comments_next.clone(),
        };
        let Some(next) = cursor else {
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
                    if let Some(index) = replacement {
                        w.comments.clear();
                        w.expanded_comments.clear();
                        w.replies.clear();
                        w.comments_replace = None;
                        for (i, sort) in w.comments_sort.iter_mut().enumerate() {
                            sort.selected = i == index;
                        }
                    }
                    w.comments.extend(cp.items);
                    w.comments_next = cp.next.filter(|c| c != &next);
                    if cp.total.is_some() {
                        w.comments_total = cp.total;
                    }
                    if !cp.sort.is_empty() {
                        w.comments_sort = cp.sort;
                    }
                    w.comments_state = CommentsState::Idle;
                    if replacement.is_some()
                        && let Some(p) = app.page_mut(id)
                    {
                        p.text.clear_comments();
                        if let Some(y) = p.layout.as_ref().and_then(|(_, l)| l.comments_y) {
                            p.scroll.jump_to(f64::from(y));
                        }
                    }
                    app.mark_if_top(id, marks::COMMENTS_LOADED);
                }
                Err(e) => {
                    w.comments_state = CommentsState::Failed(Problem::from_error(&e, "comments"))
                }
            }
        });
    }

    pub fn toggle_comment_sort(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.top_mut()
            && let Some(w) = p.watch_mut()
        {
            w.sort_open = !w.sort_open;
            p.touch();
            self.content_changed(cx);
        }
    }

    pub fn sort_comments(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(p) = self.top_mut() else { return };
        let id = p.id;
        let Some(w) = p.watch_mut() else { return };
        w.sort_open = false;
        if w.comments_sort.get(index).is_some_and(|s| !s.selected) {
            w.comments_state = CommentsState::Idle;
            w.comments_replace = Some(index);
            self.load_comments(id, cx);
        } else {
            p.touch();
        }
        self.content_changed(cx);
    }

    pub fn toggle_replies(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.top_mut() else { return };
        let Some(w) = p.watch_mut() else { return };
        let initial = w.comments.get(i).and_then(|c| c.replies.clone());
        let r = w.replies.entry(i).or_insert_with(|| Replies {
            expanded: false,
            items: Vec::new(),
            next: initial,
            status: CommentsState::NotStarted,
        });
        r.expanded = !r.expanded;
        let load = r.expanded && r.status == CommentsState::NotStarted;
        p.touch();
        if load {
            self.more_replies(i, cx);
        }
        self.content_changed(cx);
    }

    pub fn more_replies(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.top_mut() else { return };
        let (id, req) = (p.id, p.req);
        let Some(w) = p.watch_mut() else { return };
        let Some(author_id) = w.comments.get(i).map(|c| c.id.clone()) else {
            return;
        };
        let Some(r) = w.replies.get_mut(&i) else {
            return;
        };
        let Some(cursor) = r.next.clone() else { return };
        if r.status == CommentsState::Loading {
            return;
        }
        r.status = CommentsState::Loading;
        p.touch();
        let job = client().more_replies(&cursor);
        self.spawn_job(id, req, job, cx, move |app, id, result, _| {
            let Some(w) = app.page_mut(id).and_then(Page::watch_mut) else {
                return;
            };
            // A replaced sort may reuse the same row index.
            if w.comments.get(i).is_none_or(|c| c.id != author_id) {
                return;
            }
            let Some(r) = w.replies.get_mut(&i) else {
                return;
            };
            if r.next.as_ref() != Some(&cursor) || r.status != CommentsState::Loading {
                return;
            }
            match result {
                Ok(page) => {
                    r.items.extend(page.items);
                    r.next = page.next.filter(|next| next != &cursor);
                    r.status = CommentsState::Idle;
                }
                Err(e) => r.status = CommentsState::Failed(Problem::from_error(&e, "more replies")),
            }
        });
        self.content_changed(cx);
    }

    pub fn select_related(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(p) = self.top_mut() else { return };
        let (id, req) = (p.id, p.req);
        let loading_more = p.feed.loading_more;
        let Some(w) = p.watch_mut() else { return };
        let Some(info) = w.info.as_ref() else { return };
        let Some(chip) = info.related_chips.get(index) else {
            return;
        };
        if loading_more
            || w.related_pending.is_some()
            || (w.related_failed.is_none()
                && w.related_selected.map_or(chip.selected, |i| i == index))
        {
            return;
        }
        w.related_failed = None;
        let Some(cursor) = chip.next.clone() else {
            w.related_selected = Some(index);
            w.related_pending = None;
            p.feed = Feed {
                items: info.related.items.clone(),
                next: info.related.next.clone(),
                ..Default::default()
            };
            p.touch();
            self.sync_next(id, cx);
            self.content_changed(cx);
            return;
        };
        w.related_pending = Some(index);
        p.feed.loading_more = true;
        p.feed.more_error = None;
        p.touch();
        let job = client().more_videos(&cursor);
        self.spawn_job(id, req, job, cx, move |app, id, result, cx| {
            let Some(p) = app.page_mut(id) else { return };
            let Some(w) = p.watch_mut() else { return };
            if w.related_pending != Some(index) {
                return;
            }
            w.related_pending = None;
            match result {
                Ok(r) => {
                    w.related_selected = Some(index);
                    p.feed = Feed {
                        items: r.items,
                        next: r.next,
                        ..Default::default()
                    };
                    app.sync_next(id, cx);
                }
                Err(e) => {
                    w.related_failed = Some(index);
                    p.feed.loading_more = false;
                    p.feed.more_error = Some(Problem::from_error(&e, "related videos"));
                }
            }
        });
        self.content_changed(cx);
    }

    pub fn sync_next(&mut self, id: PageId, cx: &mut Context<Self>) {
        if let Some(page) = self.top().filter(|p| p.id == id && p.is_watch()) {
            let (next, previous) = successors(page);
            self.player.update(cx, |p, cx| {
                p.set_next(next, cx);
                p.set_previous(previous, cx);
            });
        }
    }

    fn advance(&mut self, v: Video, window: &mut Window, cx: &mut Context<Self>) {
        if !self.top().is_some_and(Page::is_watch) {
            return;
        }
        if library()
            .queue()
            .first()
            .is_some_and(|head| head.id == v.id)
        {
            library().pop_queued();
            self.open_video(v, window, cx);
            return;
        }
        let playlist_next = self
            .top()
            .and_then(Page::watch)
            .and_then(|w| w.info.as_ref())
            .and_then(|i| i.playlist.as_ref())
            .and_then(nt_core::PlaylistPanel::next_video);
        if playlist_next.is_some_and(|next| next.id == v.id) {
            self.play_playlist_item(v, window, cx);
        } else {
            self.open_video(v, window, cx);
        }
    }

    fn play_playlist_item(&mut self, v: Video, window: &mut Window, cx: &mut Context<Self>) {
        let playlist = self
            .top()
            .and_then(Page::watch)
            .and_then(|w| w.info.as_ref())
            .and_then(|i| i.playlist.as_ref())
            .map(|panel| panel.id.clone());
        self.open_from_playlist(v, None, playlist, window, cx);
    }

    pub fn more_watch_playlist(&mut self, id: PageId, cx: &mut Context<Self>) {
        let Some(page) = self.page_mut(id) else {
            return;
        };
        let req = page.req;
        let Some(w) = page.watch_mut() else { return };
        if w.playlist_loading {
            return;
        }
        let Some(cursor) = w
            .info
            .as_ref()
            .and_then(|i| i.playlist.as_ref())
            .and_then(|p| p.next.clone())
        else {
            return;
        };
        w.playlist_loading = true;
        w.playlist_error = false;
        page.touch();
        let job = client().more_playlist_videos(&cursor);
        self.spawn_job(id, req, job, cx, move |app, id, result, cx| {
            let Some(w) = app.page_mut(id).and_then(Page::watch_mut) else {
                return;
            };
            let Some(panel) = w.info.as_mut().and_then(|i| i.playlist.as_mut()) else {
                return;
            };
            w.playlist_loading = false;
            match result {
                Ok(page) => {
                    panel.items.extend(page.items);
                    panel.next = page.next.filter(|next| next != &cursor);
                    app.sync_next(id, cx);
                }
                Err(e) => {
                    eprintln!("nt-gpui: playlist continuation: {e}");
                    w.playlist_error = true;
                }
            }
        });
        self.content_changed(cx);
    }

    pub fn open_watch_playlist_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let data = self
            .top()
            .and_then(Page::watch)
            .and_then(|w| w.info.as_ref())
            .and_then(|i| i.playlist.as_ref())
            .and_then(|panel| {
                (!panel.index.is_some_and(|i| i == index))
                    .then(|| panel.items.get(index))
                    .flatten()
            })
            .cloned();
        if let Some(video) = data {
            self.play_playlist_item(video, window, cx);
        }
    }

    /// Retry only the page's own data (never restarts the video).
    pub fn retry_page(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.top().map(|p| p.id) {
            self.start_load(id, cx);
            self.content_changed(cx);
        }
    }

    pub fn retry_more(&mut self, cx: &mut Context<Self>) {
        if let Some(chip) = self
            .top()
            .and_then(Page::watch)
            .and_then(|w| w.related_failed)
        {
            self.select_related(chip, cx);
            return;
        }
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

    fn show_chapters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fullscreen {
            self.toggle_fullscreen(window, cx);
        }
        let Some(p) = self.top_mut() else { return };
        let y = p.layout.as_ref().and_then(|(_, layout)| {
            layout
                .columns
                .iter()
                .flatten()
                .find(|b| matches!(b.kind, BlockKind::WatchDesc { .. }))
                .map(|b| b.y)
        });
        if let Some(w) = p.watch_mut() {
            w.desc_expanded = true;
        }
        p.touch();
        if let Some(y) = y {
            p.scroll.jump_to(f64::from(y));
            self.offset = p.scroll.offset();
        }
        window.focus(&self.focus, cx);
        self.content_changed(cx);
    }

    fn toggle_fullscreen(&self, window: &mut Window, cx: &mut Context<Self>) {
        // The scroller is absent in fullscreen. Keep keyboard routing on the persistent root.
        window.focus(&self.focus, cx);
        window.toggle_fullscreen();
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
        self.clear_hover(cx);
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
        self.close_menu(cx);
        self.clear_hover(cx);
        if self.top().is_some_and(Page::is_watch) {
            self.player.update(cx, |p, cx| p.close_context(cx));
        }
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
            && (page.feed.next.is_some()
                || matches!(&page.kind, Kind::Search { view, .. } if view.transcript_next.is_some()))
            && !page.feed.loading_more
            && page.feed.more_error.is_none()
            && (lay.main_end as f64) - bottom < 1.5 * vh;
        let playlist = page.watch().and_then(|w| {
            let panel = w.info.as_ref()?.playlist.as_ref()?;
            let near_playback = panel
                .index
                .is_some_and(|i| i.saturating_add(10) >= panel.items.len());
            let near_scroll = lay.playlist_end.is_some_and(|y| f64::from(y) - bottom < vh);
            Some(
                panel.next.is_some()
                    && !w.playlist_loading
                    && !w.playlist_error
                    && (near_playback || near_scroll),
            )
        });
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
        if playlist == Some(true) {
            self.more_watch_playlist(id, cx);
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
        if key == "space" && window.focused(cx).is_some_and(|h| h != self.focus) {
            // GPUI dispatches Space/Enter as a click for the focused button.
            return;
        }
        if key == "escape" && self.menu.is_some() {
            self.close_menu(cx);
            cx.stop_propagation();
            return;
        }
        if key == "escape" && self.overlay != Overlay::None {
            self.set_overlay(Overlay::None, cx);
            cx.stop_propagation();
            return;
        }
        if key == "escape" && self.player.update(cx, |p, cx| p.close_menus(cx)) {
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
        if (key == "?" || ks.key_char.as_deref() == Some("?") || (key == "/" && m.shift))
            && !m.control
            && !m.alt
            && !m.platform
        {
            self.set_overlay(Overlay::Shortcuts, cx);
            cx.stop_propagation();
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
                self.toggle_fullscreen(window, cx);
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
            "c" => p.update(cx, |p, cx| p.toggle_captions(cx)),
            "f" => self.toggle_fullscreen(window, cx),
            "t" => self.toggle_theatre(cx),
            "home" => p.update(cx, |p, cx| p.seek_fraction(0.0, cx)),
            "end" => p.update(cx, |p, cx| p.seek_fraction(0.999, cx)),
            "," if !shift => p.update(cx, |p, cx| p.frame_step(false, cx)),
            "." if !shift => p.update(cx, |p, cx| p.frame_step(true, cx)),
            "<" | "," => p.update(cx, |p, cx| p.step_speed(false, cx)),
            ">" | "." => p.update(cx, |p, cx| p.step_speed(true, cx)),
            "n" | "N" if shift => p.update(cx, |p, cx| {
                if let Some(id) = p.source() {
                    crate::player::trace("shift-next", id);
                }
                p.play_next(cx);
            }),
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

    pub fn close_playback(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |p, _| p.detach_hardware());
        self.leave_watch(false, cx);
    }

    pub fn quit(&mut self, cx: &mut Context<Self>) {
        self.close_playback(cx);
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

fn successors(page: &Page) -> (Option<Video>, Option<Video>) {
    let panel = page
        .watch()
        .and_then(|w| w.info.as_ref())
        .and_then(|i| i.playlist.as_ref());
    let next = library()
        .queue()
        .into_iter()
        .next()
        .or_else(|| panel.and_then(nt_core::PlaylistPanel::next_video).cloned())
        .or_else(|| page.feed.items.first().cloned());
    (
        next,
        panel
            .and_then(nt_core::PlaylistPanel::previous_video)
            .cloned(),
    )
}

/// Chapters and heatmap of a watch page, for the player's scrubber.
fn page_extras(
    page: Option<&mut Page>,
) -> Option<(Vec<nt_core::Chapter>, Vec<nt_core::HeatPoint>)> {
    let info = page?.watch()?.info.as_ref()?;
    Some((info.chapters.clone(), info.heatmap.clone()))
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
        probe::begin(cx, &[Layer::Overlay, Layer::CardMenu]);
        self.preview.update(cx, |p, _| p.free_retired(window));
        self.images.begin_frame();
        if !self.top().is_some_and(Page::is_watch) {
            self.player.update(cx, |p, _| p.drop_retired_images(window));
        }
        let vp = window.viewport_size();
        let win_w = f32::from(vp.width).max(1.0);
        let geo = Geo {
            win_w,
            win_h: f32::from(vp.height).max(1.0),
            scale: window.scale_factor(),
            guide: self.prefs.guide && win_w >= GUIDE_PUSH_MIN,
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

        let player_rect = watch
            .then(|| {
                if fullscreen {
                    return Some(gpui::Bounds::new(
                        gpui::point(px(0.0), px(0.0)),
                        gpui::size(px(geo.win_w), px(geo.win_h)),
                    ));
                }
                let (x, y, w, h) = self.top()?.layout.as_ref()?.1.player?;
                let top = y - self.offset as f32;
                (top + h > 0.0 && top < ch).then(|| {
                    gpui::Bounds::new(
                        gpui::point(px((cx0 + x).round()), px((TOP_BAR_H + top).round())),
                        gpui::size(px(w.round()), px(h.round())),
                    )
                })
            })
            .flatten();
        // Where the video may show: the page area below the top bar, or the whole window.
        let video_area = if fullscreen {
            gpui::Bounds::new(
                gpui::point(px(0.0), px(0.0)),
                gpui::size(px(geo.win_w), px(geo.win_h)),
            )
        } else {
            gpui::Bounds::new(
                gpui::point(px(cx0), px(TOP_BAR_H)),
                gpui::size(px(cw), px(ch)),
            )
        };
        let native_video = self.player.update(cx, |p, cx| {
            p.sync_hardware(player_rect, video_area, window, cx)
        });
        let hole = player_rect.filter(|_| native_video);
        let background = if fullscreen { black() } else { bg() };
        let round_video = if fullscreen || self.theatre {
            0.0
        } else {
            12.0
        };

        let mut root = div()
            .id("nt-root")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .when(hole.is_none(), |d| d.bg(background))
            .children(hole.map(|hole| {
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        crate::hardware::paint_around(bounds, hole, background, round_video, window)
                    },
                )
                .absolute()
                .inset_0()
            }))
            .font_family("Roboto")
            .text_color(text())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit(cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, _, cx| this.quit(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.player.update(cx, |p, cx| p.close_menus(cx));
                }),
            )
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
                    div().absolute().inset_0().child(
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

        if geo.guide && !watch && !fullscreen {
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(TOP_BAR_H))
                    .w(px(GUIDE_W))
                    .h(px(ch))
                    .bg(bg())
                    .child(self.guide_list(cx)),
            );
        } else if rail {
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
        root = root.children(self.overlay_layer(cx));
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
            self.clear_hover(cx);
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

    fn remove_hidden(&mut self, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let library = library();
        for p in self.stack.iter_mut().chain(&mut self.forward) {
            match &mut p.kind {
                Kind::Search { view, .. } => {
                    let visible =
                        |i: usize| p.feed.items.get(i).is_some_and(|v| !library.is_hidden(v));
                    view.rows.retain(|r| match r {
                        SearchRow::Video(i) => visible(*i),
                        _ => true,
                    });
                    view.transcripts.retain(|i| visible(*i));
                }
                Kind::Home { .. } | Kind::Watch(_) => {
                    p.feed.items.retain(|v| !library.is_hidden(v));
                    p.text.clear_titles();
                }
                _ => {}
            }
            p.touch();
        }
        if let Some(id) = self.top().map(|p| p.id) {
            self.sync_next(id, cx);
        }
        self.show_toast("Recommendation removed", cx);
        self.content_changed(cx);
    }

    fn card_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let v = menu.video.clone();
        let saved = library().is_saved(&v.id);
        let channel = v.channel_id.clone().filter(|c| !c.is_empty());
        let rows = 5 + 2 * usize::from(channel.is_some());
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
                .tab_index(0)
                .focus_visible(crate::chrome::focus_ring)
                .children(target(cx, Layer::CardMenu, "menu_item", label))
                .child(crate::icons::icon(name, 24.0, text()))
                .child(txt(label, 14.0, 20.0, W400, text()))
        };
        let (v_play, v_later, id_link) = (v.clone(), v.clone(), v.id.clone());
        let v_queue = v.clone();
        let v_hide = v.clone();
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
                item("menu-queue", "playlist_add", "Add to queue").on_click(cx.listener(
                    move |this, _, _, cx| {
                        library().enqueue(&v_queue);
                        this.close_menu(cx);
                        if let Some(id) = this.top().map(|p| p.id) {
                            this.sync_next(id, cx);
                        }
                        this.show_toast("Added to queue", cx);
                    },
                )),
            )
            .child(
                item("menu-hide", "close", "Not interested").on_click(cx.listener(
                    move |this, _, _, cx| {
                        library().set_video_hidden(&v_hide.id, true);
                        this.remove_hidden(cx);
                    },
                )),
            )
            .child(
                item("menu-link", "share", "Share")
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_link(&id_link, cx))),
            );
        if let Some(cid) = channel {
            let hidden = cid.clone();
            panel = panel
                .child(
                    item("menu-hide-channel", "close", "Don't recommend channel").on_click(
                        cx.listener(move |this, _, _, cx| {
                            library().set_channel_hidden(&hidden, true);
                            this.remove_hidden(cx);
                        }),
                    ),
                )
                .child(
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
                    .tab_group()
                    .tab_index(0)
                    .tab_stop(false)
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

impl NtApp {
    pub fn probe_state(&self, window: &Window, cx: &gpui::App) -> serde_json::Value {
        use serde_json::json;
        let Some(p) = self.top() else {
            return serde_json::Value::Null;
        };
        let top = self.top_bar.read(cx);
        let player = self.player.read(cx);
        let watch = p.is_watch();
        let player_visible = watch
            && (self.fullscreen
                || p.layout
                    .as_ref()
                    .and_then(|(_, l)| l.player)
                    .is_some_and(|(_, y, _, h)| {
                        y + h > self.offset as f32
                            && y - (self.offset as f32) < self.geo.viewport_h(false)
                    }));
        let (page, results_query, topic) = match &p.kind {
            Kind::Home { topic } => (
                "home",
                None,
                p.home_chips
                    .get(*topic)
                    .map(|c| c.label.as_str())
                    .or_else(|| TOPICS.get(*topic).map(|t| t.title)),
            ),
            Kind::Search { query, .. } => (
                "search",
                (p.status == Load::Ready).then_some(query.as_str()),
                None,
            ),
            Kind::Watch(_) => ("watch", None, None),
            Kind::Channel { .. } => ("channel", None, None),
            Kind::Subscriptions { .. } => ("subscriptions", None, None),
            Kind::History => ("history", None, None),
            Kind::WatchLater => ("watch_later", None, None),
        };
        let menu = match self.overlay {
            Overlay::Guide => Some("guide"),
            Overlay::Shortcuts => Some("shortcuts"),
            Overlay::Filters => Some("filters"),
            Overlay::Playlist => Some("playlist"),
            Overlay::Queue => Some("queue"),
            Overlay::About => Some("about"),
            Overlay::None if self.menu.is_some() => Some("card"),
            Overlay::None if !self.fullscreen && top.suggestions_shown() => Some("suggestions"),
            Overlay::None => player_visible.then(|| player.popup()).flatten(),
        };
        let rail = self.geo.rail(watch) || (self.geo.guide && !watch);
        // The entry the rail or guide marks: the root page on screen (even when the window is
        // too narrow to show the rail, as the explorer's rail property expects).
        let selected = (self.stack.len() == 1)
            .then(|| p.root())
            .flatten()
            .map(|r| match r {
                Root::Home => "home",
                Root::Subscriptions => "subscriptions",
                Root::History => "history",
                Root::WatchLater => "watch_later",
            });
        let comments = p.watch();
        let error = match (
            &p.status,
            &p.feed.more_error,
            comments.map(|w| &w.comments_state),
        ) {
            (Load::Failed(e), _, _) | (_, Some(e), _) | (_, _, Some(CommentsState::Failed(e))) => {
                Some(e.title)
            }
            _ if watch && player.failed() => Some("This video can't be played"),
            _ => None,
        };
        let loading = p.status == Load::Loading
            || p.feed.loading_more
            || comments.is_some_and(|w| w.comments_state == CommentsState::Loading);
        // A stale video (the player on another id than the page) is a bug; say so in the log.
        if let Some(w) = p.watch()
            && player.source().is_some_and(|id| id != w.video.id)
        {
            eprintln!(
                "nt-gpui: probe: player has {:?}, page shows {}",
                player.source(),
                w.video.id
            );
        }
        json!({
            "page": page,
            "query": (!self.fullscreen).then(|| top.query(cx)).flatten(),
            "results_query": results_query,
            "topic": topic,
            "scroll_y": self.offset,
            "max_scroll": p.scroll.max_offset(),
            "items": match &p.kind {
                Kind::Search { view, .. } => view.rows.iter().filter(|r| !matches!(r, SearchRow::Shelf(_))).count() + view.transcripts.len(),
                Kind::Channel { info: Some(info), tab: ChannelTab::Playlists, .. } => info.playlists.items.len(),
                _ => p.feed.items.len() + usize::from(p.resume.is_some()) + comments.map_or(0, |w| {
                    w.comments.len() + w.replies.values().filter(|r| r.expanded).map(|r| r.items.len()).sum::<usize>()
                }),
            },
            "loading": loading,
            "error": error,
            "rail_selected": selected,
            "menu": menu,
            "focus": (!self.fullscreen && top.is_focused(window, cx)).then_some("search"),
            "video": watch.then(|| player.probe_state()).flatten(),
            "fullscreen": self.fullscreen,
            "theatre": self.theatre && watch,
            "back_depth": self.stack.len().saturating_sub(1),
            "forward_depth": self.forward.len(),
            "targets": probe::targets(cx, window, menu, player_visible, self.fullscreen, rail),
        })
    }
}
