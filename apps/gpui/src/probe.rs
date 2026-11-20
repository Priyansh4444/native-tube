//! Paint-derived hit rectangles for the external state explorer.
use gpui::prelude::*;
use gpui::{App, Bounds, Global, Pixels, SharedString, Window, canvas, px};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Eq, PartialEq, Clone, Copy)]
pub enum Layer {
    Chrome,
    Suggestions,
    Rail,
    Page,
    Player,
    Settings,
    PlayerMenu,
    Stats,
    CardMenu,
    Overlay,
}

#[derive(Clone)]
struct Target {
    layer: Layer,
    kind: &'static str,
    bounds: Bounds<Pixels>,
    label: SharedString,
}

#[derive(Default)]
pub struct Probe(Rc<RefCell<Vec<Target>>>);

impl Global for Probe {}

pub fn begin(cx: &App, layers: &[Layer]) {
    if let Some(probe) = cx.try_global::<Probe>() {
        probe.0.borrow_mut().retain(|t| !layers.contains(&t.layer));
    }
}

pub fn target<L: Into<SharedString>>(
    cx: &App,
    layer: Layer,
    kind: &'static str,
    label: L,
) -> Option<impl IntoElement + use<L>> {
    let probe = cx.try_global::<Probe>()?.0.clone();
    let label = label.into();
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let bounds = bounds.intersect(&window.content_mask().bounds);
                if bounds.size.width > px(0.0) && bounds.size.height > px(0.0) {
                    probe.borrow_mut().push(Target {
                        layer,
                        kind,
                        bounds,
                        label,
                    });
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

pub fn targets(
    cx: &App,
    window: &Window,
    menu: Option<&str>,
    player_visible: bool,
    fullscreen: bool,
    rail: bool,
) -> Vec<Value> {
    use Layer::*;
    let Some(probe) = cx.try_global::<Probe>() else {
        return Vec::new();
    };
    let targets = probe.0.borrow();
    let drawn = |layer| match layer {
        Page | Chrome => !fullscreen,
        Rail => rail && !fullscreen,
        Player => player_visible,
        Settings => player_visible && menu == Some("settings"),
        PlayerMenu => player_visible && menu == Some("player"),
        Stats => player_visible && menu == Some("stats"),
        Suggestions => menu == Some("suggestions"),
        CardMenu => menu == Some("card"),
        Overlay => matches!(
            menu,
            Some("guide" | "shortcuts" | "filters" | "playlist" | "queue" | "about")
        ),
    };
    let modal = match menu {
        Some("card") => Some(CardMenu),
        Some("guide" | "shortcuts" | "filters" | "playlist" | "queue" | "about") => Some(Overlay),
        _ => None,
    };
    targets
        .iter()
        .filter(|t| drawn(t.layer) && modal.is_none_or(|l| t.layer == l))
        .filter(|t| menu != Some("player") || t.layer != Player)
        .filter(|t| !t.kind.starts_with('_'))
        .filter(|t| {
            t.kind != "more"
                || targets.iter().any(|row| {
                    matches!(row.kind, "card" | "row" | "related")
                        && row.bounds.intersects(&t.bounds)
                        && row.bounds.contains(&window.mouse_position())
                })
        })
        .filter_map(|t| {
            let mut b = t.bounds;
            for blocker in targets
                .iter()
                .filter(|o| o.kind == "_panel" && o.layer != t.layer && drawn(o.layer))
            {
                if b.intersects(&blocker.bounds) {
                    b = uncovered(b, blocker.bounds)?;
                }
            }
            Some(
                json!({"kind":t.kind, "rect":[f32::from(b.origin.x), f32::from(b.origin.y),
                f32::from(b.size.width), f32::from(b.size.height)], "label":t.label.as_ref()}),
            )
        })
        .take(80)
        .collect()
}

/// Use the largest rectangular part still clickable when a popup covers one edge.
fn uncovered(b: Bounds<Pixels>, cover: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let i = b.intersect(&cover);
    let (x, y) = (b.left(), b.top());
    [
        Bounds::from_corners(gpui::point(x, y), gpui::point(b.right(), i.top())),
        Bounds::from_corners(
            gpui::point(x, i.bottom()),
            gpui::point(b.right(), b.bottom()),
        ),
        Bounds::from_corners(gpui::point(x, i.top()), gpui::point(i.left(), i.bottom())),
        Bounds::from_corners(
            gpui::point(i.right(), i.top()),
            gpui::point(b.right(), i.bottom()),
        ),
    ]
    .into_iter()
    .filter(|r| r.size.width > px(0.0) && r.size.height > px(0.0))
    .max_by_key(|r| (f32::from(r.size.width) * f32::from(r.size.height)) as u64)
}

pub fn spawn(window: &mut Window, cx: &mut gpui::Context<crate::app::NtApp>) {
    if cx.try_global::<Probe>().is_none() {
        return;
    }
    let start = Instant::now();
    cx.spawn_in(window, async move |this, cx| {
        let mut last = Value::Null;
        let mut written = None::<Instant>;
        loop {
            let Ok(state) = this.update_in(cx, |app, window, cx| app.probe_state(window, cx))
            else {
                break;
            };
            if state != last || written.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
                last = state;
                let mut line = last.clone();
                line["t_ms"] = json!(start.elapsed().as_millis() as u64);
                eprintln!("NT_STATE {line}");
                written = Some(Instant::now());
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
    })
    .detach();
}
