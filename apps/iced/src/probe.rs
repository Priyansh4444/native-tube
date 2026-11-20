//! The state-probe contract is sampled at draw time, after layout and scroll animation.
use crate::app::{App, Body, Comments, Feed, Load, More, Root};
use iced::{Rectangle, Vector};
use nt_core::{Status, TOPICS};
use serde::Serialize;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Serialize)]
struct Target {
    kind: &'static str,
    rect: [f32; 4],
    label: String,
}

#[derive(Default)]
struct Drawing {
    offset: Vector,
    /// Visible bounds of the iced `scrollable`s being drawn, innermost last.
    scopes: Vec<Rectangle>,
    targets: Vec<Target>,
    search_focused: bool,
}

thread_local! {
    static DRAWING: RefCell<Drawing> = RefCell::new(Drawing::default());
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NT_PROBE").as_deref() == Ok("1"))
}

pub fn begin() {
    if enabled() {
        DRAWING.with(|d| {
            let mut d = d.borrow_mut();
            d.targets.clear();
            d.search_focused = false;
        });
    }
}

pub fn search_focused() {
    if enabled() {
        DRAWING.with(|d| d.borrow_mut().search_focused = true);
    }
}

pub fn translated(offset: Vector, draw: impl FnOnce()) {
    if enabled() {
        DRAWING.with(|d| d.borrow_mut().offset += offset);
        draw();
        DRAWING.with(|d| d.borrow_mut().offset -= offset);
    } else {
        draw();
    }
}

/// Draw the children of an iced `scrollable` whose visible bounds are `bounds`. The scrollable
/// translates its content and hands children a viewport moved by the same amount, so the
/// scroll offset is the viewport's distance from these bounds.
pub fn scoped(bounds: Rectangle, draw: impl FnOnce()) {
    if enabled() {
        DRAWING.with(|d| d.borrow_mut().scopes.push(bounds));
        draw();
        DRAWING.with(|d| d.borrow_mut().scopes.pop());
    } else {
        draw();
    }
}

pub fn target(kind: &'static str, label: &str, bounds: Rectangle, viewport: Rectangle) {
    if enabled()
        && let Some(rect) = bounds.intersection(&viewport)
    {
        DRAWING.with(|d| {
            let mut d = d.borrow_mut();
            let scroll = d.scopes.last().map_or(Vector::ZERO, |s| {
                Vector::new(s.x - viewport.x, s.y - viewport.y)
            });
            let rect = rect + d.offset + scroll;
            d.targets.push(Target {
                kind,
                rect: [rect.x, rect.y, rect.width, rect.height],
                label: label.into(),
            });
        });
    }
}

pub fn cover(bounds: Rectangle) {
    if enabled() {
        DRAWING.with(|d| {
            let mut d = d.borrow_mut();
            let bounds = bounds + d.offset;
            d.targets.retain_mut(|target| {
                let [x, y, width, height] = target.rect;
                let rect = Rectangle {
                    x,
                    y,
                    width,
                    height,
                };
                let Some(hit) = rect.intersection(&bounds) else {
                    return true;
                };
                // Keep the largest remaining clickable part of a partially covered target.
                let parts = [
                    Rectangle {
                        width: hit.x - x,
                        ..rect
                    },
                    Rectangle {
                        x: hit.x + hit.width,
                        width: x + width - hit.x - hit.width,
                        ..rect
                    },
                    Rectangle {
                        height: hit.y - y,
                        ..rect
                    },
                    Rectangle {
                        y: hit.y + hit.height,
                        height: y + height - hit.y - hit.height,
                        ..rect
                    },
                ];
                let Some(r) = parts
                    .into_iter()
                    .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)))
                else {
                    return false;
                };
                target.rect = [r.x, r.y, r.width, r.height];
                r.width > 0.0 && r.height > 0.0
            });
        });
    }
}

#[derive(Default)]
pub struct Probe {
    last: Cell<Option<Instant>>,
    previous: RefCell<Value>,
}

impl Probe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn draw(&self, app: &App) {
        if !enabled() {
            return;
        }
        let now = Instant::now();
        let elapsed = self
            .last
            .get()
            .map_or(Duration::MAX, |last| now.saturating_duration_since(last));
        if elapsed < Duration::from_millis(100) {
            return;
        }
        let Some(mut snapshot) = snapshot(app) else {
            return;
        };
        let mut previous = self.previous.borrow_mut();
        if elapsed < Duration::from_secs(1) && snapshot == *previous {
            return;
        }
        *previous = snapshot.clone();
        self.last.set(Some(now));
        snapshot["t_ms"] = json!((crate::clock::secs(now) * 1000.0) as u64);
        eprintln!("NT_STATE {snapshot}");
    }
}

fn load<'a>(load: &'a Load, feed: &'a Feed) -> (usize, bool, Option<&'a str>) {
    (
        feed.items.len(),
        *load == Load::Loading || feed.more == Some(More::Loading),
        match (load, &feed.more) {
            (Load::Failed(e), _) | (_, Some(More::Failed(e))) => Some(e.as_str()),
            _ => None,
        },
    )
}

fn snapshot(app: &App) -> Option<Value> {
    let page = app.top()?;
    let mut query = None;
    let mut topic = None;
    let mut video = None;
    let (name, (items, loading, error)) = match &page.body {
        Body::Home(h) => {
            topic = h
                .chips
                .get(h.topic)
                .map(|t| t.label.as_str())
                .or_else(|| TOPICS.get(h.topic).map(|t| t.title));
            ("home", load(&h.load, &h.feed))
        }
        Body::Search(s) => {
            query = Some(s.query.as_str());
            let (n, loading, error) = load(&s.load, &s.feed);
            let (m, more, more_error) = load(&s.load, &s.moments);
            let mixed = if s.entries.is_empty() {
                n
            } else {
                s.entries
                    .iter()
                    .filter(|e| !matches!(e, crate::app::SearchEntry::Shelf(_)))
                    .count()
            };
            ("search", (mixed + m, loading || more, error.or(more_error)))
        }
        Body::Channel(c) => {
            let (mut n, mut loading, mut error) = load(&c.load, &c.feed);
            if c.tab == nt_core::ChannelTab::Playlists {
                n = c.info.as_ref().map_or(0, |i| i.playlists.items.len());
                loading |= c.playlists_more == More::Loading;
                if let More::Failed(e) = &c.playlists_more {
                    error = Some(e);
                }
            }
            loading |= c.desc_open && c.about_load == Load::Loading;
            ("channel", (n, loading, error))
        }
        Body::Subs(s) => ("subscriptions", load(&s.load, &s.feed)),
        Body::History(v) => ("history", (v.len(), false, None)),
        Body::Later(v) => ("watch_later", (v.len(), false, None)),
        Body::Watch(w) => {
            let status = match (&app.play.status, &app.player_error) {
                (Status::Failed(_), _) | (_, Some(_)) => "failed",
                (Status::Idle, _) => "idle",
                (Status::Loading, _) => "loading",
                (Status::Ready, _) => "ready",
                (Status::Ended, _) => "ended",
            };
            video = Some(json!({
                "id": w.video.id, "status": status, "position": app.play.position,
                "duration": app.play.duration, "paused": app.play.paused, "muted": app.play.muted,
                "first_frame": w.first_frame.get(),
            }));
            let error = app.player_error.as_deref().or(
                match (&w.load, &w.comments_state, &app.play.status) {
                    (Load::Failed(e), _, _) | (_, Comments::Failed(e), _) => Some(e.as_str()),
                    (_, _, Status::Failed(_)) => Some("This video can't be played right now."),
                    _ => None,
                },
            );
            (
                "watch",
                (
                    w.related.len()
                        + w.playlist.as_ref().map_or(0, |p| p.items.len())
                        + w.comments.len()
                        + w.replies.values().map(|r| r.items.len()).sum::<usize>(),
                    w.load == Load::Loading
                        || w.comments_state == Comments::Loading
                        || w.related_more == More::Loading
                        || w.playlist_more == More::Loading
                        || w.replies.values().any(|r| r.more == Some(More::Loading)),
                    error
                        .or_else(|| {
                            [&w.playlist_more, &w.related_more]
                                .into_iter()
                                .find_map(|more| match more {
                                    More::Failed(e) => Some(e.as_str()),
                                    _ => None,
                                })
                        })
                        .or_else(|| {
                            w.replies.values().find_map(|replies| match &replies.more {
                                Some(More::Failed(e)) if replies.open => Some(e.as_str()),
                                _ => None,
                            })
                        }),
                ),
            )
        }
    };
    let menu = if app.queue_open {
        Some("queue")
    } else if app.search_filters {
        Some("filters")
    } else if app.shortcuts {
        Some("shortcuts")
    } else if app.card_menu.is_some() {
        Some("card")
    } else if app.player_context.is_some() {
        Some("player")
    } else if app.guide_is_overlay() {
        Some("guide")
    } else if app.sugg_open && !app.suggestions.is_empty() {
        Some("suggestions")
    } else if app.menu.is_some() {
        Some("settings")
    } else if app.stats && name == "watch" {
        Some("stats")
    } else if app.up_next.is_some() && name == "watch" && app.next_video().is_some() {
        Some("up_next")
    } else {
        None
    };
    // Truthful to the frame just drawn: a selection only where rail entries were drawn
    // (the window's width can change between building the view and drawing it).
    let rail_drawn = DRAWING.with(|d| d.borrow().targets.iter().any(|t| t.kind == "rail"));
    let rail_selected = rail_drawn
        .then(|| app.shown_root())
        .flatten()
        .map(|root| match root {
            Root::Home => "home",
            Root::Subscriptions => "subscriptions",
            Root::History => "history",
            Root::WatchLater => "watch_later",
        });
    let (targets, focus) = DRAWING.with(|d| {
        let drawing = d.borrow();
        (
            json!(&drawing.targets[..drawing.targets.len().min(80)]),
            drawing.search_focused.then_some("search"),
        )
    });
    let model = if app.fullscreen {
        &app.fullscreen_scroll
    } else {
        &page.scroll
    };
    Some(json!({
        "page": name, "query": (!app.fullscreen && (app.win.width >= 656.0 || app.search_open)).then_some(&app.query),
        "results_query": query, "topic": topic, "scroll_y": model.offset(),
        "max_scroll": model.max_offset(), "items": items, "loading": loading, "error": error,
        "rail_selected": rail_selected, "menu": menu, "focus": focus,
        "video": video, "fullscreen": app.fullscreen, "theatre": name == "watch" && app.prefs.theatre && !app.fullscreen,
        "back_depth": app.pages.len() - 1, "forward_depth": app.forward_depth(), "targets": targets,
    }))
}
