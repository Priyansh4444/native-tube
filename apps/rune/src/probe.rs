//! The app state probe (docs/testing/state-probe.md): with `NT_PROBE=1`, one `NT_STATE {json}`
//! line on stderr per state change (at most one per 100 ms) and a heartbeat every second.
//!
//! Off by default; `tick` returns at once unless the variable is set, and the JSON is only built
//! when it is. Rune's after-draw inspection supplies the visible, ancestor-clipped rectangles.

use crate::app::App;
use crate::pages::*;
use crate::theme::*;
use crate::video::Menu;
use nt_core::Status;
use rune::{El, ElementInfo, Runtime, Tx};
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MIN_GAP: f64 = 0.1;
const HEARTBEAT: f64 = 1.0;
const MAX_TARGETS: usize = 80;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NT_PROBE").is_ok_and(|v| v == "1"))
}

fn t0() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

pub fn init() {
    if enabled() {
        t0();
    }
}

#[derive(Default)]
pub struct Probe {
    last: String,
    drawn_state: String,
    last_at: f64,
    scene: (u64, i64, i64, Option<rune::time::Instant>),
    targets: Vec<Target>,
}

type Target = (&'static str, [f32; 4], Option<String>);

pub fn target(el: El, kind: &'static str) -> El {
    if enabled() {
        el.trace_namespace("nt_probe", kind).trace_name(kind)
    } else {
        el
    }
}

/// Inspect the submitted scene, including live scroll transforms and built-in scroll views.
pub fn drawn(rt: &Runtime<App>) {
    let s = &rt.state;
    let hardware = s.native_picture();
    if (hardware || s.video.shown.has_picture)
        && !s.video.first_shown.get()
        && rt
            .presented(if hardware { "player" } else { "picture" })
            .is_some_and(|([x, y, w, h], opacity)| {
                opacity > 0.0
                    && x + w > 0.0
                    && x < s.win_w
                    && y < s.win_h
                    && y + h > if s.fullscreen { 0.0 } else { TOP_BAR }
            })
    {
        s.video.first_shown.set(true);
        nt_core::metrics::mark(nt_core::metrics::marks::PLAYBACK_STARTED);
    }
    if !enabled() {
        return;
    }
    let now = t0().elapsed().as_secs_f64();
    let mut probe = s.probe.borrow_mut();
    let scene = (
        s.scene_version.get(),
        (s.top().scroll.anim.offset() * s.scale.get() as f64).round() as i64,
        s.chips_x.round() as i64,
        rune::last_input(),
    );
    if scene != probe.scene || probe.last.is_empty() {
        probe.targets = presented_targets(rt);
        probe.scene = scene;
    }
    probe.drawn_state = state(s, &probe.targets);
    emit(&mut probe, now);
}

pub fn tick(s: &App, tx: &mut Tx) {
    if !enabled() {
        return;
    }
    let now = t0().elapsed().as_secs_f64();
    let mut probe = s.probe.borrow_mut();
    let same_scene = probe.scene.0 == s.scene_version.get() && !probe.last.is_empty();
    let elapsed = now - probe.last_at;
    if same_scene && s.native_picture() && elapsed >= MIN_GAP {
        // The Wayland child presents independently after the controls hide.
        probe.drawn_state = state(s, &probe.targets);
    }
    if elapsed >= MIN_GAP && !probe.drawn_state.is_empty() {
        emit(&mut probe, now);
    }
    let interval = if s.native_picture() || probe.last != probe.drawn_state {
        MIN_GAP
    } else {
        HEARTBEAT
    };
    tx.wake_after(Duration::from_secs_f64(
        (interval - (now - probe.last_at)).max(0.01),
    ));
}

fn emit(probe: &mut Probe, now: f64) {
    let since = now - probe.last_at;
    if !probe.last.is_empty() && since < MIN_GAP {
        return;
    }
    if probe.drawn_state != probe.last || since >= HEARTBEAT {
        eprintln!(
            "NT_STATE {{\"t_ms\":{},{}",
            (now * 1000.0) as u64,
            probe.drawn_state
        );
        probe.last.clone_from(&probe.drawn_state);
        probe.last_at = now;
    }
}

fn presented_targets(rt: &Runtime<App>) -> Vec<Target> {
    let els = rt.elements();
    let s = &rt.state;
    let modal = [
        (s.shortcuts, "shortcuts-layer"),
        (s.queue_open, "queue-layer"),
        (s.guide_over, "guide-layer"),
        (s.card_menu.is_some(), "menu-layer"),
        (s.player_menu.is_some(), "player-menu-layer"),
        (s.search_filters_open, "search-filters-layer"),
        (s.video.menu != Menu::Closed, "settings-layer"),
    ]
    .into_iter()
    .find_map(|(open, key)| open.then_some(key));
    let scope = modal.and_then(|key| {
        els.iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
    });
    let occluders: Vec<_> = els
        .iter()
        .filter(|e| !e.exiting && matches!(e.key.as_deref(), Some("suggest" | "stats" | "toast")))
        .collect();
    let inside = |path: &str, parent: &str| {
        path.strip_prefix(parent)
            .is_some_and(|tail| tail.starts_with('/'))
    };
    let mut targets = Vec::new();
    for e in &els {
        if e.exiting
            || e.inert
            || e.opacity <= 0.05
            || scope.is_some_and(|p| !inside(&e.path, &p.path))
        {
            continue;
        }
        let Some(kind) = target_kind(e) else { continue };
        let [x, y, w, h] = e.visible;
        let bounds = [
            x.max(0.0),
            y.max(0.0),
            (x + w).min(s.win_w),
            (y + h).min(s.win_h),
        ];
        let mut rects = vec![bounds];
        for cover in &occluders {
            if !inside(&e.path, &cover.path) {
                let [x, y, w, h] = cover.visible;
                rects = rects
                    .into_iter()
                    .flat_map(|r| subtract(r, [x, y, x + w, y + h]))
                    .collect();
            }
        }
        let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        if let Some([x0, y0, x1, y1]) = rects
            .into_iter()
            .filter(|r| r[2] - r[0] >= 2.0 && r[3] - r[1] >= 2.0)
            .max_by(|a, b| area(a).total_cmp(&area(b)))
        {
            targets.push((kind, [x0, y0, x1 - x0, y1 - y0], e.label.clone()));
        }
        if targets.len() == MAX_TARGETS {
            break;
        }
    }
    targets
}

fn subtract(r: [f32; 4], c: [f32; 4]) -> Vec<[f32; 4]> {
    let [x0, y0, x1, y1] = r;
    let [l, t, r, b] = [x0.max(c[0]), y0.max(c[1]), x1.min(c[2]), y1.min(c[3])];
    if l >= r || t >= b {
        return vec![[x0, y0, x1, y1]];
    }
    vec![
        [x0, y0, x1, t],
        [x0, b, x1, y1],
        [x0, t, l, b],
        [r, t, x1, b],
    ]
}

fn target_kind(e: &ElementInfo) -> Option<&'static str> {
    if let Some(("nt_probe", kind)) = e.trace_namespace
        && e.trace_name == Some(kind)
    {
        return Some(kind);
    }
    if e.role == "menuitem" {
        return Some("menu_item");
    }
    let key = e.key.as_deref()?;
    Some(match key {
        "guide" | "guide-close" if e.role == "button" && e.label.is_some() => "guide",
        "back" if e.role == "button" => "back",
        "search-open" | "search-go" => "search_button",
        "input" if e.path.contains("searchbox") => "search_field",
        "more" => "more",
        "player" => "player",
        "play" => "play",
        "next" => "next",
        "previous" => "previous",
        "mute" => "mute",
        "cc" => "captions",
        "scrubber" => "scrubber",
        "settings" => "settings",
        "theatre" => "theatre",
        "fullscreen" => "fullscreen",
        "subscribe" => "subscribe",
        "later" => "watch_later",
        "share" => "share",
        "retry" | "retry-play" => "retry",
        "shortcuts-close" | "stats-close" => "close",
        k if k.starts_with("rail-") || k.starts_with("guide-") && e.role == "link" => "rail",
        k if k.starts_with("chip-") => "chip",
        _ => return None,
    })
}

fn esc(out: &mut String, v: &str) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn opt(out: &mut String, v: Option<&str>) {
    match v {
        Some(v) => esc(out, v),
        None => out.push_str("null"),
    }
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{:.1}", v)
    } else {
        "0".into()
    }
}

fn load_err(l: &Load) -> Option<&str> {
    match l {
        Load::Error(e) => Some(e.as_str()),
        _ => None,
    }
}

fn page_id(kind: &Kind) -> &'static str {
    match kind {
        Kind::Home { .. } => "home",
        Kind::Search(_) => "search",
        Kind::Watch(_) => "watch",
        Kind::Channel { .. } => "channel",
        Kind::Subs { .. } => "subscriptions",
        Kind::History => "history",
        Kind::Later => "watch_later",
    }
}

fn rail_selected(kind: &Kind, selected: bool) -> Option<&'static str> {
    selected.then(|| page_id(kind))
}

/// Everything after `t_ms`, closing brace included.
fn state(s: &App, targets: &[Target]) -> String {
    let page = s.top();

    let mut o = String::with_capacity(2048);
    let name = page_id(&page.kind);
    o.push_str("\"page\":");
    esc(&mut o, name);
    o.push_str(",\"query\":");
    esc(&mut o, &s.search.text);
    o.push_str(",\"results_query\":");
    let sr = match &page.kind {
        Kind::Search(sr) => Some(sr),
        _ => None,
    };
    opt(&mut o, sr.map(|sr| sr.query.as_str()));
    o.push_str(",\"topic\":");
    let topic = match page.kind {
        Kind::Home { topic } => page
            .home_chip
            .and_then(|i| page.home_chips.get(i))
            .map(|c| c.label.as_str())
            .or_else(|| nt_core::TOPICS.get(topic).map(|t| t.title)),
        _ => None,
    };
    opt(&mut o, topic);
    let offset = page.scroll.anim.offset();
    let max = page.scroll.anim.max_offset();
    let _ = write!(
        o,
        ",\"scroll_y\":{},\"max_scroll\":{}",
        num(offset),
        num(max)
    );
    let w = page.watch();
    let channel_playlists = match &page.kind {
        Kind::Channel {
            info: Some(info), ..
        } if page.channel_tab == Some(nt_core::ChannelTab::Playlists) => info.playlists.items.len(),
        _ => 0,
    };
    let items = channel_playlists
        + sr.map_or(page.feed.items.len(), |sr| {
            sr.entries.len() + sr.trans.items.len()
        })
        + w.map_or(0, |w| {
            w.comments.len()
                + w.replies.values().map(|r| r.items.len()).sum::<usize>()
                + w.playlist.as_ref().map_or(0, |panel| panel.items.len())
        });
    let loading = page.playlist_cards_more == Load::Loading
        || page.feed.status == Load::Loading
        || page.feed.more == Load::Loading
        || w.is_some_and(|w| {
            w.comments_state == Load::Loading
                || w.playlist_more == Load::Loading
                || (w.info.is_none() && w.error.is_none())
                || w.replies.values().any(|r| r.state == Load::Loading)
        })
        || sr.is_some_and(|sr| sr.trans.more == Load::Loading);
    let _ = write!(o, ",\"items\":{items},\"loading\":{loading}");
    let error = load_err(&page.feed.status)
        .or_else(|| load_err(&page.playlist_cards_more))
        .or_else(|| w.and_then(|w| w.error.as_deref()))
        .or_else(|| load_err(&page.feed.more))
        .or_else(|| {
            w.and_then(|w| load_err(&w.comments_state).or_else(|| load_err(&w.playlist_more)))
        })
        .or_else(|| sr.and_then(|sr| load_err(&sr.trans.more)))
        .or_else(|| {
            w.and(match (&s.video.error, &s.video.shown.status) {
                (Some(_), _) => Some("This video can't be played right now."),
                (_, Status::Failed(_)) => Some("This video is unavailable."),
                _ => None,
            })
        });
    o.push_str(",\"error\":");
    opt(&mut o, error);
    o.push_str(",\"rail_selected\":");
    let side = page.has_rail(s.win_w) && !s.fullscreen;
    opt(
        &mut o,
        rail_selected(&page.kind, side && crate::view::nav_selected(s).is_some()),
    );
    o.push_str(",\"menu\":");
    let menu = if s.shortcuts {
        Some("shortcuts")
    } else if s.card_menu.is_some() {
        Some("card")
    } else if s.player_menu.is_some() {
        Some("player")
    } else if s.video.menu != Menu::Closed && s.video.controls && w.is_some() {
        Some("settings")
    } else if s.guide_over {
        Some("guide")
    } else if s.queue_open {
        Some("queue")
    } else if s.search_filters_open {
        Some("filters")
    } else if s.sugg_open {
        Some("suggestions")
    } else if s.video.stats_open && w.is_some() {
        Some("stats")
    } else {
        None
    };
    opt(&mut o, menu);
    o.push_str(",\"focus\":");
    opt(&mut o, s.search.focused.then_some("search"));
    o.push_str(",\"video\":");
    match (w, s.video.id.as_deref()) {
        (Some(_), Some(id)) if s.video.active => {
            let v = &s.video;
            let status = match v.shown.status {
                Status::Idle => "idle",
                Status::Loading => "loading",
                Status::Ready => "ready",
                Status::Ended => "ended",
                Status::Failed(_) => "failed",
            };
            let status = if v.error.is_some() { "failed" } else { status };
            o.push_str("{\"id\":");
            esc(&mut o, id);
            o.push_str(",\"status\":");
            esc(&mut o, status);
            let _ = write!(
                o,
                ",\"position\":{},\"duration\":{},\"paused\":{},\"muted\":{},\"first_frame\":{}}}",
                num(v.position),
                num(v.duration),
                v.shown.paused,
                v.shown.muted,
                v.first_shown.get()
            );
        }
        _ => o.push_str("null"),
    }
    let _ = write!(
        o,
        ",\"fullscreen\":{},\"theatre\":{},\"back_depth\":{},\"forward_depth\":{},\"targets\":[",
        s.fullscreen,
        s.prefs.theatre && w.is_some(),
        s.back_depth(),
        s.forward_depth()
    );
    let mut first = true;
    for (kind, r, label) in targets {
        if !first {
            o.push(',');
        }
        first = false;
        o.push_str("{\"kind\":");
        esc(&mut o, kind);
        let _ = write!(
            o,
            ",\"rect\":[{},{},{},{}],\"label\":",
            num(r[0] as f64),
            num(r[1] as f64),
            num(r[2] as f64),
            num(r[3] as f64)
        );
        opt(&mut o, label.as_deref());
        o.push('}');
    }
    o.push_str("]}");
    o
}

#[cfg(test)]
mod tests {
    use super::rail_selected;
    use crate::view::NAV;

    #[test]
    fn rail_selected_uses_page_ids() {
        let ids = NAV.map(|(_, _, _, make_page)| rail_selected(&make_page(), true));
        assert_eq!(
            ids,
            ["home", "subscriptions", "history", "watch_later"].map(Some)
        );
        let make_home = NAV[0].3;
        assert_eq!(rail_selected(&make_home(), false), None);
    }
}
