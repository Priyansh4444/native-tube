use super::*;
use crate::model::{TargetId, Video};

fn state(page: &str, depth: usize) -> State {
    State {
        t_ms: 0,
        page: page.into(),
        query: None,
        results_query: None,
        topic: None,
        scroll_y: 0.0,
        max_scroll: 100.0,
        items: 10,
        loading: false,
        error: None,
        rail_selected: (page == "home").then(|| "home".into()),
        menu: None,
        focus: None,
        video: None,
        fullscreen: false,
        theatre: false,
        back_depth: depth,
        forward_depth: 0,
        targets: Vec::new(),
    }
}

fn key(key: &str) -> Action {
    Action::Key { key: key.into() }
}

fn target(kind: &str) -> TargetId {
    TargetId {
        kind: kind.into(),
        label: "Example".into(),
        occurrence: 0,
    }
}

fn video(id: &str, first_frame: bool) -> Video {
    Video {
        id: id.into(),
        status: "ready".into(),
        position: 1.0,
        duration: 2.0,
        paused: false,
        muted: true,
        first_frame,
    }
}

#[test]
fn back_restores_search_snapshot_with_four_pixel_tolerance() {
    let mut engine = Properties::default();
    engine.observe(0, state("home", 0)).unwrap();
    let mut search = state("search", 1);
    search.results_query = Some("rust".into());
    engine.observe(20, search.clone()).unwrap();
    search.scroll_y = 96.0;
    engine.observe(30, search.clone()).unwrap();
    engine.observe(40, state("watch", 2)).unwrap();
    engine.action(50, &key("alt+Left")).unwrap();

    let mut incorrect = search.clone();
    incorrect.scroll_y = 91.0;
    engine.observe(100, incorrect).unwrap();
    assert!(!engine.settled());
    search.scroll_y = 92.0;
    engine.observe(300, search).unwrap();
    assert!(engine.settled());
}

#[test]
fn back_requires_the_previous_video_id_and_deadline() {
    let mut engine = Properties::default();
    let mut watch = state("watch", 0);
    watch.video = Some(video("first", true));
    engine.observe(0, watch.clone()).unwrap();
    let mut next = state("watch", 1);
    next.video = Some(video("second", true));
    engine.observe(100, next.clone()).unwrap();
    engine.action(150, &key("alt+Left")).unwrap();
    next.back_depth = 0;
    engine.observe(500, next).unwrap();
    let error = engine.observe(2151, watch).unwrap_err();
    assert_eq!(error.property, Property::BackRestoration);
}

#[test]
fn watch_requires_a_new_nonempty_id_and_presented_frame() {
    let mut engine = Properties::default();
    let mut watch = state("watch", 0);
    watch.video = Some(video("old", true));
    engine.observe(0, watch).unwrap();
    engine
        .action(
            50,
            &Action::Click {
                target: target("related"),
                right: false,
            },
        )
        .unwrap();

    let mut next = state("watch", 1);
    next.video = Some(video("old", true));
    engine.observe(300, next.clone()).unwrap();
    assert!(!engine.settled());
    next.video = Some(video("", true));
    engine.observe(400, next.clone()).unwrap();
    assert!(!engine.settled());
    next.video = Some(video("new", false));
    engine.observe(800, next.clone()).unwrap();
    assert!(!engine.settled());
    next.video = Some(video("new", true));
    engine.observe(6050, next).unwrap();
    assert!(engine.settled());
}

#[test]
fn escape_and_search_use_independent_deadlines() {
    let mut engine = Properties::default();
    let mut home = state("home", 0);
    home.menu = Some("settings".into());
    engine.observe(0, home.clone()).unwrap();
    engine.action(100, &key("Escape")).unwrap();
    let error = engine.observe(601, home).unwrap_err();
    assert_eq!(error.property, Property::EscapeDismissesMenu);

    let mut engine = Properties::default();
    engine.observe(0, state("home", 0)).unwrap();
    engine
        .action(
            200,
            &Action::Search {
                target: target("search_field"),
                text: "rust".into(),
            },
        )
        .unwrap();
    let mut search = state("search", 1);
    search.query = Some("rust".into());
    search.results_query = Some("old".into());
    engine.observe(600, search.clone()).unwrap();
    assert!(!engine.settled());
    search.results_query = search.query.clone();
    engine.observe(6200, search).unwrap();
    assert!(engine.settled());
}

#[test]
fn every_action_requires_loading_to_end() {
    let mut engine = Properties::default();
    engine.observe(0, state("home", 0)).unwrap();
    engine.action(10, &key("t")).unwrap();
    engine.observe(30, state("home", 0)).unwrap();
    let mut home = state("home", 0);
    home.loading = true;
    engine.observe(500, home).unwrap();
    assert_eq!(
        engine.check_deadlines(10011).unwrap_err().property,
        Property::LoadingEnds
    );
}

#[test]
fn rejects_scroll_rail_pss_and_process_violations() {
    let mut engine = Properties::default();
    let mut search = state("search", 0);
    search.scroll_y = f64::NAN;
    assert_eq!(
        engine.observe(0, search.clone()).unwrap_err().property,
        Property::ScrollBounds
    );
    search.scroll_y = 101.0;
    assert_eq!(
        engine.observe(0, search.clone()).unwrap_err().property,
        Property::ScrollBounds
    );
    search.scroll_y = 100.0;
    search.rail_selected = Some("home".into());
    assert_eq!(
        engine.observe(0, search).unwrap_err().property,
        Property::RailSelection
    );
    assert!(health(true, Some(1_464_843)).is_ok());
    assert_eq!(
        health(true, Some(1_464_844)).unwrap_err().property,
        Property::Pss
    );
    assert_eq!(health(false, None).unwrap_err().property, Property::Process);
}

#[test]
fn rail_selection_is_required_only_where_the_rail_is_drawn() {
    use crate::model::Target;
    let rail = |label: &str| Target {
        kind: "rail".into(),
        rect: [0.0, 60.0, 72.0, 72.0],
        label: label.into(),
    };
    // A narrow startup frame without the rail truthfully selects nothing (iced at 1172 ms).
    let mut home = state("home", 0);
    home.rail_selected = None;
    assert!(Properties::default().observe(0, home.clone()).is_ok());
    // Once the rail is drawn, Home must be the selected entry, by page id.
    home.targets = vec![rail("Home"), rail("Subscriptions")];
    let error = Properties::default().observe(0, home.clone()).unwrap_err();
    assert_eq!(error.property, Property::RailSelection);
    home.rail_selected = Some("Home".into());
    let error = Properties::default().observe(0, home.clone()).unwrap_err();
    assert_eq!(error.property, Property::RailSelection);
    home.rail_selected = Some("home".into());
    assert!(Properties::default().observe(0, home).is_ok());
    // Search never selects an entry, drawn rail or not.
    let mut search = state("search", 0);
    search.targets = vec![rail("Home")];
    assert!(Properties::default().observe(0, search.clone()).is_ok());
    search.rail_selected = Some("home".into());
    let error = Properties::default().observe(0, search).unwrap_err();
    assert_eq!(error.property, Property::RailSelection);
}
